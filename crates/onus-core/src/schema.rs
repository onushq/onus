//! JSON Schemas for the formats Onus reads and writes.

use schemars::schema_for;

use crate::change::SemanticReport;
use crate::config::OnusConfig;
use crate::map::CodebaseMap;
use crate::plugins::PluginsFile;
use crate::protocol::PluginProtocol;
use crate::provenance::ProvenanceRecord;
use crate::signal::{ProductionSignal, RolloutMetrics};

/// `(file name, pretty JSON)` for every published schema, in a fixed order.
pub fn all_schemas() -> Vec<(&'static str, String)> {
    let mut out = vec![
        (
            "codebase-map.schema.json",
            serde_json::to_string_pretty(&schema_for!(CodebaseMap)),
        ),
        (
            "semantic-report.schema.json",
            serde_json::to_string_pretty(&schema_for!(SemanticReport)),
        ),
        (
            "onus-config.schema.json",
            serde_json::to_string_pretty(&schema_for!(OnusConfig)),
        ),
        (
            "onus-plugins.schema.json",
            serde_json::to_string_pretty(&schema_for!(PluginsFile)),
        ),
        (
            "plugin-protocol.schema.json",
            serde_json::to_string_pretty(&schema_for!(PluginProtocol)),
        ),
        (
            "provenance.schema.json",
            serde_json::to_string_pretty(&schema_for!(ProvenanceRecord)),
        ),
        (
            "production-signal.schema.json",
            serde_json::to_string_pretty(&schema_for!(ProductionSignal)),
        ),
        (
            "rollout-metrics.schema.json",
            serde_json::to_string_pretty(&schema_for!(RolloutMetrics)),
        ),
    ]
    .into_iter()
    .map(|(name, json)| (name, json.expect("schemas serialize")))
    .collect::<Vec<_>>();
    for (_, json) in &mut out {
        json.push('\n');
    }
    out
}
