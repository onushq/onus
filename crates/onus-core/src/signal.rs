//! Production signals (`schemas/production-signal.schema.json`): what
//! production says about changes once they ship. Incidents, error spikes,
//! latency and business metrics, rollbacks, deploys, rollout stages and
//! observed calls. A monitoring or deploy tool writes them in this one
//! vendor-neutral shape; `onus signals ingest` keeps each once in the
//! records (`signals.jsonl`), ties it to a change, and turns incidents and
//! rollbacks into outcomes (`onus help production`).
//!
//! The input and the stored line are the same type. On ingest Onus fills in
//! `at`, `change` and `attributedBy`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One production signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProductionSignal {
    /// Unique per source (`pagerduty:Q1ABC`, `sentry:4711`): a signal with an
    /// id already on record is not recorded again.
    pub id: String,
    pub kind: SignalKind,
    /// Unix seconds; now when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// The system it came from (`pagerduty`, `datadog`, `argo`, `manual`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    /// The change it is about, `owner/repo#123`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    /// A commit sha, or a prefix of at least 7 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// An opaque deploy id, for grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy: Option<String>,
    /// Where it happened (`production`, `staging`, …). Absent means
    /// production.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Component ids of the map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Symbol ids (`billing:src/a.ts#charge`); their components count too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// For error-spike, latency and metric: what moved. For rollout: the
    /// samples Onus judged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<MetricSample>,
    /// For trace: the caller and callee pairs observed (at most 5000).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<ObservedCall>,
    /// For deploy: the share of traffic it reached. For rollout: the stage
    /// and Onus's decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout: Option<RolloutStage>,
    /// Written by Onus: `change` or `commit`, how the signal was tied to its
    /// change. Absent when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributed_by: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SignalKind {
    /// Something broke in production. Attributed, it becomes an `incident`
    /// outcome of its change.
    Incident,
    /// Error rates rose.
    ErrorSpike,
    /// Responses slowed.
    Latency,
    /// A business or other metric moved.
    Metric,
    /// The change was rolled back. Attributed, it becomes a `rolled-back`
    /// outcome of its change.
    Rollback,
    /// A rollout stage and Onus's decision. Written only by `onus ci
    /// rollout`; `onus signals ingest` refuses it.
    Rollout,
    /// A deploy tool says a commit reached an environment, or a share of
    /// its traffic.
    Deploy,
    /// Calls observed in production traces, folded into the map.
    Trace,
}

impl SignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::Incident => "incident",
            SignalKind::ErrorSpike => "error-spike",
            SignalKind::Latency => "latency",
            SignalKind::Metric => "metric",
            SignalKind::Rollback => "rollback",
            SignalKind::Rollout => "rollout",
            SignalKind::Deploy => "deploy",
            SignalKind::Trace => "trace",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

/// One measurement, and what it is compared with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MetricSample {
    /// `error_rate`, `latency_p95_ms`, `checkout_conversion`, …
    pub name: String,
    pub value: f64,
    /// The control or pre-deploy value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Where a call happened in code: the OpenTelemetry code attributes, free of
/// any map.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CodeRef {
    /// `service.name`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
    /// `code.filepath`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// `code.function`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// `code.namespace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// `code.lineno`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A caller and callee seen together in a trace: a span and its parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObservedCall {
    pub from: CodeRef,
    pub to: CodeRef,
    /// How many times it was seen. Default 1.
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

/// A share of production traffic, and for a rollout stage, Onus's decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RolloutStage {
    /// 0 to 100.
    pub percent: u8,
    /// `promote`, `hold`, `rollback` or `complete`: set only by `onus ci
    /// rollout`. A deploy has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// The feature flag the change ships behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
}

/// What `onus ci rollout --metrics` reads (`schemas/rollout-metrics.schema.json`):
/// the samples of one rollout stage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RolloutMetrics {
    pub metrics: Vec<MetricSample>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_round_trip_and_refuse_typos() {
        let text = r#"{"id":"pagerduty:Q1ABC","kind":"incident","at":1759990000,"source":"pagerduty","severity":"high","commit":"3f9a1c2","components":["billing"],"note":"checkout 500s"}"#;
        let s: ProductionSignal = serde_json::from_str(text).unwrap();
        assert_eq!(s.kind, SignalKind::Incident);
        assert_eq!(s.severity, Some(Severity::High));
        assert_eq!(serde_json::to_string(&s).unwrap(), text);
        let trace = ProductionSignal {
            id: "trace:1".into(),
            kind: SignalKind::Trace,
            at: None,
            source: String::new(),
            severity: None,
            change: None,
            commit: None,
            deploy: None,
            environment: None,
            components: vec![],
            symbols: vec![],
            note: None,
            metrics: vec![],
            calls: vec![ObservedCall {
                from: CodeRef {
                    service: "orders".into(),
                    file: Some("src/a.ts".into()),
                    function: Some("a".into()),
                    ..CodeRef::default()
                },
                to: CodeRef {
                    service: "billing".into(),
                    ..CodeRef::default()
                },
                count: 3,
            }],
            rollout: None,
            attributed_by: None,
        };
        let line = serde_json::to_string(&trace).unwrap();
        assert_eq!(
            serde_json::from_str::<ProductionSignal>(&line).unwrap(),
            trace
        );
        // A call seen once needs no count.
        let call: ObservedCall = serde_json::from_str(r#"{"from":{},"to":{}}"#).unwrap();
        assert_eq!(call.count, 1);
        assert!(
            serde_json::from_str::<ProductionSignal>(
                r#"{"id":"x","kind":"incident","severty":"high"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<ProductionSignal>(r#"{"id":"x","kind":"outage"}"#).is_err());
        let deploy: ProductionSignal = serde_json::from_str(
            r#"{"id":"d","kind":"deploy","commit":"3f9a1c2","environment":"production","rollout":{"percent":25}}"#,
        )
        .unwrap();
        assert_eq!(deploy.rollout.unwrap().decision, None);
    }

    #[test]
    fn severity_is_ordered() {
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
        assert_eq!(SignalKind::ErrorSpike.as_str(), "error-spike");
        assert_eq!(
            serde_json::to_string(&SignalKind::ErrorSpike).unwrap(),
            "\"error-spike\""
        );
    }
}
