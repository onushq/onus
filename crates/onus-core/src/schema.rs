//! JSON Schemas for the formats Onus reads and writes.

use schemars::schema_for;

use crate::change::SemanticReport;
use crate::config::OnusConfig;
use crate::held_out::HeldOutSuite;
use crate::map::CodebaseMap;
use crate::plugins::PluginsFile;
use crate::protocol::PluginProtocol;
use crate::provenance::{EscalationDecision, ProvenanceRecord};
use crate::signal::{ProductionSignal, RolloutMetrics};

/// The input of `--escalations FILE`: a JSON array, titled as the format it
/// is rather than as the generator names a list.
fn escalation_decisions() -> schemars::Schema {
    let mut s = schema_for!(Vec<EscalationDecision>);
    s.insert("title".into(), "EscalationDecisions".into());
    s.insert(
        "description".into(),
        "Escalations as Onus Cloud keeps them, with their decisions: the input of \
         `onus provenance --escalations FILE` and `onus ci pr --escalations FILE`. \
         Also read: the API's `{\"escalations\": [...]}`, a stored row's `status` \
         (for `state`) and `input: {scopes, reason}`, and a view's nested \
         `decision: {by: {id, name}, at, answer}`."
            .into(),
    );
    s
}

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
            "escalation-decisions.schema.json",
            serde_json::to_string_pretty(&escalation_decisions()),
        ),
        (
            "production-signal.schema.json",
            serde_json::to_string_pretty(&schema_for!(ProductionSignal)),
        ),
        (
            "rollout-metrics.schema.json",
            serde_json::to_string_pretty(&schema_for!(RolloutMetrics)),
        ),
        (
            "onus-held-out.schema.json",
            serde_json::to_string_pretty(&schema_for!(HeldOutSuite)),
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
