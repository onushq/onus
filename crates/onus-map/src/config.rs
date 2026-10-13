//! Loading and validating `onus.yaml`.

use std::path::Path;

use onus_core::hash::sha256_hex;
use onus_core::{Lane, OnusConfig, RolloutConfig, RolloutStrategy, Thresholds};

use crate::MapError;

/// The config file name at the root of a tree.
pub const CONFIG_FILE: &str = "onus.yaml";

/// A parsed config plus the hash of its bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: OnusConfig,
    /// sha256 of the config and every pack it lists.
    pub hash: String,
    /// The texts of the packs listed in `extractors.packs`, read when the
    /// config is loaded, so both trees of a diff use the same packs.
    pub packs: Vec<String>,
}

pub fn parse(text: &str) -> Result<LoadedConfig, MapError> {
    let config: OnusConfig =
        serde_yaml_ng::from_str(text).map_err(|e| MapError::Config(e.to_string()))?;
    validate(&config)?;
    Ok(LoadedConfig {
        config,
        hash: sha256_hex(text.as_bytes()),
        packs: vec![],
    })
}

pub fn load(path: &Path) -> Result<LoadedConfig, MapError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| MapError::Io(format!("{}: {e}", path.display())))?;
    load_text(&text, path)
}

/// Loads a config from `text` as if it were the file at `path`: packs are
/// read relative to its folder, and errors name it. Nothing is written.
pub fn load_text(text: &str, path: &Path) -> Result<LoadedConfig, MapError> {
    let mut loaded = parse(text).map_err(|e| match e {
        MapError::Config(msg) => MapError::Config(format!("{}: {msg}", path.display())),
        other => other,
    })?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut hashed = text.to_string();
    for pack in &loaded.config.extractors.packs {
        let pack_path = onus_core::paths::native(dir, pack);
        let pack_text = std::fs::read_to_string(&pack_path)
            .map_err(|e| MapError::Config(format!("{}: pack `{pack}`: {e}", path.display())))?;
        hashed.push('\0');
        hashed.push_str(&pack_text);
        loaded.packs.push(pack_text);
    }
    loaded.hash = sha256_hex(hashed.as_bytes());
    Ok(loaded)
}

/// Loads `<root>/onus.yaml` if it exists.
pub fn load_from_tree(root: &Path) -> Result<Option<LoadedConfig>, MapError> {
    let path = root.join(CONFIG_FILE);
    if path.is_file() {
        load(&path).map(Some)
    } else {
        Ok(None)
    }
}

pub fn validate(config: &OnusConfig) -> Result<(), MapError> {
    let mut problems = Vec::new();
    if config.version != 1 {
        problems.push(format!("version must be 1, found {}", config.version));
    }
    for (id, c) in &config.components {
        if id.is_empty() || id.contains([':', '#', ' ', '/']) {
            problems.push(format!(
                "component id `{id}` may not be empty or contain `:`, `#`, `/` or spaces"
            ));
        }
        if c.path.to_vec().iter().any(|p| p.trim().is_empty()) {
            problems.push(format!("component `{id}` has an empty path"));
        }
        for p in c.path.to_vec() {
            if let Err(e) = globset::Glob::new(&p) {
                problems.push(format!("component `{id}`: invalid path glob `{p}`: {e}"));
            }
        }
    }
    for p in &config.test_data {
        if let Err(e) = globset::Glob::new(p) {
            problems.push(format!("testData: invalid glob `{p}`: {e}"));
        }
    }
    let known = |c: &str| config.components.is_empty() || config.components.contains_key(c);
    for (i, rule) in config.rules.iter().enumerate() {
        let from = &rule.deny.from;
        let to = rule
            .deny
            .to
            .strip_suffix(".internal")
            .unwrap_or(&rule.deny.to);
        for c in [from.as_str(), to] {
            if !known(c) {
                problems.push(format!("rule {}: unknown component `{c}`", i + 1));
            }
        }
        if from == to {
            problems.push(format!(
                "rule {}: `from` and `to` are the same component",
                i + 1
            ));
        }
    }
    for (name, contract) in &config.contracts {
        if !contract.symbol.contains(':') || !contract.symbol.contains('#') {
            problems.push(format!(
                "contract `{name}`: symbol must look like `<component>:<path>#<name>`"
            ));
        }
    }
    if let Some(lanes) = &config.lanes {
        let inputs = &lanes.inputs;
        for (key, t) in [
            ("blastRadius", &inputs.blast_radius),
            ("history.reverts", &inputs.history.reverts),
            ("history.incidents", &inputs.history.incidents),
        ] {
            thresholds(&format!("lanes.inputs.{key}"), t, &mut problems);
        }
        if !(1..=3650).contains(&inputs.history.days) {
            problems.push(format!(
                "lanes.inputs.history.days must be between 1 and 3650, found {}",
                inputs.history.days
            ));
        }
        if let Some(learned) = &lanes.learned {
            let (judge, human) = (learned.judge, learned.human);
            if !(0.0 < judge && judge <= human && human <= 1.0) {
                problems.push(format!(
                    "lanes.learned: the thresholds must be 0 < judge <= human <= 1, found judge {judge} and human {human}"
                ));
            }
            if !learned.model.ends_with("lanes-model.json") {
                problems.push(
                    "lanes.learned.model: name the file …lanes-model.json, so a change to it goes to a person"
                        .into(),
                );
            }
        }
        if let Some(policy) = &lanes.rollout {
            for lane in [Lane::AutoMerge, Lane::Judge, Lane::Human] {
                if let Some(r) = policy.get(lane) {
                    rollout(
                        &format!("lanes.rollout.{}", lane.as_str()),
                        r,
                        &mut problems,
                    );
                }
            }
        }
    }
    if let Some(events) = &config.extractors.events {
        for p in events.publish.iter().chain(&events.subscribe) {
            if !p.contains("$EVENT") {
                problems.push(format!("event pattern `{p}` must contain `$EVENT`"));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(MapError::Config(problems.join("; ")))
    }
}

/// Steps ascend strictly from 1 to 100 and end at 100; `none` has none;
/// every guard sets a limit.
fn rollout(key: &str, r: &RolloutConfig, problems: &mut Vec<String>) {
    let steps = &r.steps;
    if r.strategy == RolloutStrategy::None {
        if !steps.is_empty() {
            problems.push(format!("{key}: strategy none takes no steps"));
        }
    } else {
        if steps.last() != Some(&100) {
            problems.push(format!("{key}: the steps must end at 100, found {steps:?}"));
        }
        if steps.iter().any(|s| !(1..=100).contains(s)) {
            problems.push(format!(
                "{key}: each step is a share of traffic from 1 to 100, found {steps:?}"
            ));
        }
        if steps.windows(2).any(|w| w[1] <= w[0]) {
            problems.push(format!("{key}: the steps must go up, found {steps:?}"));
        }
    }
    for g in &r.guards {
        if g.max.is_none() && g.min.is_none() && g.max_ratio.is_none() && g.min_ratio.is_none() {
            problems.push(format!(
                "{key}: the guard on {} sets no limit (max, min, maxRatio or minRatio)",
                g.metric
            ));
        }
    }
}

/// Thresholds are at least 1 and ascend with the lane.
fn thresholds(key: &str, t: &Thresholds, problems: &mut Vec<String>) {
    let set: Vec<(Lane, u32)> = [Lane::Judge, Lane::Human, Lane::Blocked]
        .into_iter()
        .filter_map(|l| t.get(l).map(|n| (l, n)))
        .collect();
    for (lane, n) in &set {
        if *n == 0 {
            problems.push(format!("{key}: {} must be at least 1", lane.as_str()));
        }
    }
    for pair in set.windows(2) {
        let ((low, a), (high, b)) = (pair[0], pair[1]);
        if b < a {
            problems.push(format!(
                "{key}: {} ({b}) is below {} ({a})",
                high.as_str(),
                low.as_str()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_plan_example() {
        let text = r#"
version: 1
components:
  billing: { path: "services/billing/**", labels: [payments] }
  notifications: { path: "services/notifications/**", owners: ["@team-growth"] }
rules:
  - deny: { from: billing, to: notifications.internal }
labels:
  payments: { sensitivity: high }
extractors:
  events:
    publish: ["bus.publish($EVENT, ...)"]
  externals:
    "@acme/sms": { vendor: "Acme SMS", category: sms, egress: [phone] }
"#;
        let loaded = parse(text).unwrap();
        assert_eq!(loaded.config.components.len(), 2);
        assert_eq!(loaded.config.rules[0].deny.to, "notifications.internal");
        assert_eq!(loaded.hash.len(), 64);
    }

    #[test]
    fn rejects_unknown_fields_and_components() {
        assert!(parse("version: 1\ncomponents: {}\nfoo: 1\n").is_err());
        let bad_rule = "version: 1\ncomponents:\n  a: { path: \"a/**\" }\nrules:\n  - deny: { from: a, to: b.internal }\n";
        let err = parse(bad_rule).unwrap_err().to_string();
        assert!(err.contains("unknown component `b`"), "{err}");
        assert!(parse("version: 2\n").is_err());
    }

    #[test]
    fn risk_input_thresholds_ascend_from_one() {
        let err = |lanes: &str| {
            parse(&format!("version: 1\nlanes:\n  inputs: {lanes}\n"))
                .unwrap_err()
                .to_string()
        };
        assert!(
            err("{ blastRadius: { judge: 3, human: 2 } }")
                .contains("lanes.inputs.blastRadius: human (2) is below judge (3)")
        );
        assert!(
            err("{ history: { incidents: { human: 0 } } }")
                .contains("lanes.inputs.history.incidents: human must be at least 1")
        );
        assert!(err("{ history: { days: 0 } }").contains("history.days"));
        assert!(
            parse("version: 1\nlanes:\n  inputs: { blastRadius: {}, history: { days: 30 } }\n")
                .is_ok()
        );
    }

    #[test]
    fn a_learned_model_has_ordered_thresholds_and_a_named_file() {
        let err = |learned: &str| {
            parse(&format!("version: 1\nlanes:\n  learned: {learned}\n"))
                .unwrap_err()
                .to_string()
        };
        assert!(
            err("{ judge: 0.9, human: 0.5 }").contains(
                "lanes.learned: the thresholds must be 0 < judge <= human <= 1, found judge 0.9 and human 0.5"
            )
        );
        assert!(err("{ judge: 0 }").contains("0 < judge"));
        assert!(err("{ model: model.json }").contains(
            "lanes.learned.model: name the file …lanes-model.json, so a change to it goes to a person"
        ));
        let loaded = parse("version: 1\nlanes:\n  learned: {}\n").unwrap();
        let learned = loaded.config.lanes.unwrap().learned.unwrap();
        assert_eq!(
            (learned.model.as_str(), learned.judge, learned.human),
            ("onus.lanes-model.json", 0.6, 0.85)
        );
        assert!(parse("version: 1\nlanes:\n  learned: { model: ops/lanes-model.json }\n").is_ok());
    }

    #[test]
    fn rollout_plans_have_ascending_steps_that_end_at_100() {
        let rollout = |lane: &str, plan: &str| {
            parse(&format!(
                "version: 1\nlanes:\n  rollout:\n    {lane}: {plan}\n"
            ))
        };
        let ok = rollout(
            "judge",
            "{ strategy: canary, steps: [5, 25, 100], bakeMinutes: 30, guards: [{ metric: error_rate, maxRatio: 1.5 }, { metric: checkout_conversion, minRatio: 0.97 }] }",
        )
        .unwrap();
        let policy = ok.config.lanes.unwrap().rollout.unwrap();
        assert_eq!(policy.judge.unwrap().steps, [5, 25, 100]);
        assert!(rollout("auto-merge", "{ strategy: none }").is_ok());
        let err = |lane: &str, plan: &str| rollout(lane, plan).unwrap_err().to_string();
        assert!(
            err("judge", "{ strategy: canary, steps: [25, 5, 100] }")
                .contains("lanes.rollout.judge: the steps must go up, found [25, 5, 100]")
        );
        assert!(
            err("human", "{ strategy: flag, steps: [1, 5, 25] }")
                .contains("lanes.rollout.human: the steps must end at 100, found [1, 5, 25]")
        );
        assert!(
            err("auto-merge", "{ strategy: none, steps: [100] }")
                .contains("lanes.rollout.auto-merge: strategy none takes no steps")
        );
        assert!(err("judge", "{ strategy: canary, steps: [0, 100] }").contains("from 1 to 100"));
        assert!(
            err(
                "judge",
                "{ strategy: canary, steps: [100], guards: [{ metric: error_rate }] }"
            )
            .contains("the guard on error_rate sets no limit")
        );
        assert!(rollout("blocked", "{ strategy: none }").is_err());
    }
}
