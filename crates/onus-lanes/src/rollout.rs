//! Rollout plans (ADR 0016): how a change meets production traffic, by its
//! lane. A built-in plan per lane, which `lanes.rollout` replaces lane by
//! lane; a stricter plan for a change in a component that had incidents,
//! reverts or rollbacks lately; and the decision on each stage a deploy tool
//! reports. Onus decides and never moves traffic: the deploy tool acts on
//! the decision.

use onus_core::signal::MetricSample;
use onus_core::{Lane, MetricGuard, RolloutConfig, RolloutPolicy, RolloutStrategy, SemanticReport};
use serde::{Deserialize, Serialize};

use crate::classify::{AreaHistories, AreaHistory, area};

/// How one change should meet production traffic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutPlan {
    /// The lane the plan is for.
    pub lane: Lane,
    pub strategy: RolloutStrategy,
    /// For `flag`: a suggested flag key, `onus-<owner>-<repo>-<n>`.
    pub flag: Option<String>,
    /// Shares of traffic in percent, in order.
    pub steps: Vec<u8>,
    /// How long each step runs healthy before the next.
    pub bake_minutes: u32,
    pub guards: Vec<MetricGuard>,
    /// Whether a breached guard rolls the change back.
    pub automatic_rollback: bool,
    /// The components the change touches.
    pub components: Vec<String>,
    /// Why the plan is what it is, the lane's plan first.
    pub reasons: Vec<String>,
}

fn guard(metric: &str, max_ratio: f64) -> MetricGuard {
    MetricGuard {
        metric: metric.into(),
        max: None,
        min: None,
        max_ratio: Some(max_ratio),
        min_ratio: None,
    }
}

/// The built-in plan of a lane; none for `blocked`.
pub fn defaults(lane: Lane) -> Option<RolloutConfig> {
    let plan = |strategy, steps: &[u8], bake_minutes, error_rate, latency| RolloutConfig {
        strategy,
        steps: steps.to_vec(),
        bake_minutes,
        guards: vec![
            guard("error_rate", error_rate),
            guard("latency_p95_ms", latency),
        ],
    };
    match lane {
        Lane::AutoMerge => Some(plan(RolloutStrategy::Canary, &[10, 100], 10, 2.0, 1.5)),
        Lane::Judge => Some(plan(RolloutStrategy::Canary, &[5, 25, 100], 30, 1.5, 1.3)),
        Lane::Human => Some(plan(RolloutStrategy::Flag, &[1, 5, 25, 100], 120, 1.2, 1.2)),
        Lane::Blocked => None,
    }
}

/// The policy's plan for the lane, else the built-in one.
pub fn config_for(lane: Lane, policy: Option<&RolloutPolicy>) -> Option<RolloutConfig> {
    policy
        .and_then(|p| p.get(lane))
        .cloned()
        .or_else(|| defaults(lane))
}

/// The touched components that had incidents, reverts or rollbacks.
pub fn troubled<'a>(
    areas: &'a AreaHistories,
    components: &[String],
) -> Vec<(&'a str, &'a AreaHistory)> {
    areas
        .iter()
        .filter(|(c, h)| components.contains(c) && h.incidents + h.reverts > 0)
        .map(|(c, h)| (c.as_str(), h))
        .collect()
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "`billing` had 1 incident and 1 revert or rollback in the last 90 days"
fn trouble(component: &str, h: &AreaHistory, days: u32) -> String {
    let mut what = Vec::new();
    if h.incidents > 0 {
        what.push(plural(h.incidents, "incident", "incidents"));
    }
    if h.reverts > 0 {
        what.push(plural(
            h.reverts,
            "revert or rollback",
            "reverts or rollbacks",
        ));
    }
    format!(
        "`{component}` had {} in the last {days} days",
        what.join(" and ")
    )
}

/// A flag key for the change: `onus-acme-shop-42`, else `onus-<head>`.
pub fn flag_key(change: Option<&str>, head: &str) -> String {
    let raw = match change {
        Some(c) => c.to_string(),
        None => head.chars().take(12).collect(),
    };
    let mut key = String::from("onus");
    for part in raw
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
    {
        key.push('-');
        key.push_str(part);
    }
    key
}

fn stricter(lane: Lane) -> Lane {
    match lane {
        Lane::AutoMerge => Lane::Judge,
        Lane::Judge => Lane::Human,
        other => other,
    }
}

/// The plan for a change in `lane`. A change that touches a component with
/// incidents, reverts or rollbacks in the history window gets the next
/// stricter lane's plan; in the human lane, twice the bake time.
pub fn plan(
    lane: Lane,
    report: &SemanticReport,
    change: Option<&str>,
    policy: Option<&RolloutPolicy>,
    areas: &AreaHistories,
    history_days: u32,
) -> RolloutPlan {
    let mut components: Vec<String> = report
        .changes
        .iter()
        .filter_map(area)
        .map(str::to_string)
        .collect();
    components.sort();
    components.dedup();
    let flag = change.map_or_else(|| flag_key(None, &report.head), |c| flag_key(Some(c), ""));
    plan_for(lane, components, flag, policy, areas, history_days)
}

/// [`plan`] for a change known by its components and flag key, without its
/// report.
pub fn plan_for(
    lane: Lane,
    components: Vec<String>,
    flag: String,
    policy: Option<&RolloutPolicy>,
    areas: &AreaHistories,
    history_days: u32,
) -> RolloutPlan {
    let Some(mut config) = config_for(lane, policy) else {
        return RolloutPlan {
            lane,
            strategy: RolloutStrategy::None,
            flag: None,
            steps: vec![],
            bake_minutes: 0,
            guards: vec![],
            automatic_rollback: false,
            components,
            reasons: vec!["a blocked change may not ship".into()],
        };
    };
    let mut reasons = vec![format!("the {} lane's plan", lane.as_str())];
    let trouble_found = troubled(areas, &components);
    if !trouble_found.is_empty() {
        let why = trouble_found
            .iter()
            .map(|(c, h)| trouble(c, h, history_days))
            .collect::<Vec<_>>()
            .join("; ");
        match lane {
            Lane::AutoMerge | Lane::Judge => {
                let to = stricter(lane);
                if let Some(c) = config_for(to, policy) {
                    config = c;
                }
                reasons.push(format!("{why}: the {} lane's plan instead", to.as_str()));
            }
            _ => {
                config.bake_minutes = config.bake_minutes.saturating_mul(2);
                reasons.push(format!(
                    "{why}: {} min at each step instead",
                    config.bake_minutes
                ));
            }
        }
    }
    let strategy = config.strategy;
    RolloutPlan {
        lane,
        strategy,
        flag: (strategy == RolloutStrategy::Flag).then_some(flag),
        steps: config.steps,
        bake_minutes: config.bake_minutes,
        automatic_rollback: strategy != RolloutStrategy::None && !config.guards.is_empty(),
        guards: config.guards,
        components,
        reasons,
    }
}

/// What to do after a rollout stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// Healthy and baked: go to the next step.
    Promote,
    /// Not baked yet, or a guard has no sample: stay.
    Hold,
    /// A guard is breached: roll back.
    Rollback,
    /// Healthy and baked at 100%: done.
    Complete,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Promote => "promote",
            Decision::Hold => "hold",
            Decision::Rollback => "rollback",
            Decision::Complete => "complete",
        }
    }
}

/// Onus's decision on one stage of a change's rollout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageDecision {
    pub change: String,
    pub percent: u8,
    pub decision: Decision,
    /// The next step's share of traffic, on `promote`.
    pub next: Option<u8>,
    pub reasons: Vec<String>,
}

/// Numbers as people write them: `0.031`, `1.5`, `200`.
fn num(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The decision on the stage at `percent`, from the samples taken there.
/// A breached guard rolls back, whatever else holds; a guard with no
/// sample or baseline holds; then the stage must bake for the plan's time
/// since it started.
pub fn evaluate(
    change: &str,
    plan: &RolloutPlan,
    percent: u8,
    samples: &[MetricSample],
    stage_started: u64,
    now: u64,
) -> StageDecision {
    let mut breaches = Vec::new();
    let mut holds = Vec::new();
    for g in &plan.guards {
        let Some(s) = samples.iter().find(|s| s.name == g.metric) else {
            holds.push(format!("no {} sample", g.metric));
            continue;
        };
        let v = s.value;
        if let Some(max) = g.max
            && v > max
        {
            breaches.push(format!("{} {} is above {}", g.metric, num(v), num(max)));
        }
        if let Some(min) = g.min
            && v < min
        {
            breaches.push(format!("{} {} is below {}", g.metric, num(v), num(min)));
        }
        if g.max_ratio.is_none() && g.min_ratio.is_none() {
            continue;
        }
        let Some(base) = s.baseline.filter(|b| *b != 0.0) else {
            holds.push(format!("{} has no baseline to compare with", g.metric));
            continue;
        };
        let ratio = v / base;
        if let Some(limit) = g.max_ratio
            && ratio > limit
        {
            breaches.push(format!(
                "{} {} is {}× its baseline {} (limit {}×)",
                g.metric,
                num(v),
                num((ratio * 10.0).round() / 10.0),
                num(base),
                num(limit)
            ));
        }
        if let Some(limit) = g.min_ratio
            && ratio < limit
        {
            breaches.push(format!(
                "{} {} is {}× its baseline {} (at least {}×)",
                g.metric,
                num(v),
                num((ratio * 100.0).round() / 100.0),
                num(base),
                num(limit)
            ));
        }
    }
    let decide = |decision: Decision, next: Option<u8>, reasons: Vec<String>| StageDecision {
        change: change.to_string(),
        percent,
        decision,
        next,
        reasons,
    };
    if !breaches.is_empty() {
        return decide(Decision::Rollback, None, breaches);
    }
    if !holds.is_empty() {
        return decide(Decision::Hold, None, holds);
    }
    let baked = now.saturating_sub(stage_started);
    let bake = u64::from(plan.bake_minutes) * 60;
    if baked < bake {
        return decide(
            Decision::Hold,
            None,
            vec![format!("baked {} of {} min", baked / 60, plan.bake_minutes)],
        );
    }
    let healthy = if plan.guards.is_empty() {
        "no guards to check".to_string()
    } else {
        format!(
            "{} within limits",
            plan.guards
                .iter()
                .map(|g| g.metric.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if percent >= 100 {
        return decide(Decision::Complete, None, vec![healthy]);
    }
    let next = plan
        .steps
        .iter()
        .copied()
        .find(|s| *s > percent)
        .unwrap_or(100);
    decide(
        Decision::Promote,
        Some(next),
        vec![format!("{healthy}; baked {} min", plan.bake_minutes)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{ReportSummary, StructureNotes, TextStats};

    fn report(components: &[&str]) -> SemanticReport {
        let row = |c: &str| {
            serde_json::from_value::<onus_core::SemanticChange>(serde_json::json!({
                "id": format!("internal-changes:{c}"), "kind": "internal",
                "subkind": "internal-changes", "level": "structure", "subject": c,
                "component": c, "kindLabel": "", "title": "x", "whyItMatters": "",
                "hints": {"labels": [], "blastRadius": 0, "novelty": [], "confidence": "static",
                          "rulesOfTheGame": false, "intentMismatch": false, "needsPerson": false},
                "locations": []
            }))
            .unwrap()
        };
        SemanticReport {
            schema_version: 1,
            base: "base".into(),
            head: "3f9a1c2b7d4e5f60".into(),
            summary: ReportSummary {
                meaning_changes: 0,
                needs_attention: 0,
                secrets: 0,
                new_rule_violations: 0,
                intent_mismatches: 0,
            },
            changes: components.iter().map(|c| row(c)).collect(),
            intent_check: None,
            rule_violations: vec![],
            structure: StructureNotes::default(),
            text_stats: TextStats::default(),
            map_diagnostics: vec![],
        }
    }

    fn quiet() -> AreaHistories {
        AreaHistories::new()
    }

    #[test]
    fn each_lane_has_a_built_in_plan() {
        let r = report(&["orders"]);
        let p = |lane| plan(lane, &r, Some("acme/shop#42"), None, &quiet(), 90);
        let auto = p(Lane::AutoMerge);
        assert_eq!(
            (auto.strategy, auto.steps.as_slice(), auto.bake_minutes),
            (RolloutStrategy::Canary, &[10, 100][..], 10)
        );
        assert_eq!(auto.guards[0].max_ratio, Some(2.0));
        assert_eq!(auto.flag, None);
        let judge = p(Lane::Judge);
        assert_eq!(judge.steps, [5, 25, 100]);
        assert_eq!(judge.bake_minutes, 30);
        assert_eq!(
            (judge.guards[0].metric.as_str(), judge.guards[1].max_ratio),
            ("error_rate", Some(1.3))
        );
        assert!(judge.automatic_rollback);
        assert_eq!(judge.components, ["orders"]);
        assert_eq!(judge.reasons, ["the judge lane's plan"]);
        let human = p(Lane::Human);
        assert_eq!(human.strategy, RolloutStrategy::Flag);
        assert_eq!(human.steps, [1, 5, 25, 100]);
        assert_eq!(human.flag.as_deref(), Some("onus-acme-shop-42"));
        let blocked = p(Lane::Blocked);
        assert_eq!(blocked.strategy, RolloutStrategy::None);
        assert!(blocked.steps.is_empty() && !blocked.automatic_rollback);
        assert_eq!(blocked.reasons, ["a blocked change may not ship"]);
        assert!(defaults(Lane::Blocked).is_none());
        // Without a change, the flag is named after the head.
        let h = plan(Lane::Human, &r, None, None, &quiet(), 90);
        assert_eq!(h.flag.as_deref(), Some("onus-3f9a1c2b7d4e"));
    }

    #[test]
    fn a_policy_replaces_only_its_lane() {
        let policy = RolloutPolicy {
            judge: Some(RolloutConfig {
                strategy: RolloutStrategy::Canary,
                steps: vec![50, 100],
                bake_minutes: 5,
                guards: vec![],
            }),
            ..RolloutPolicy::default()
        };
        let r = report(&["orders"]);
        let judge = plan(Lane::Judge, &r, None, Some(&policy), &quiet(), 90);
        assert_eq!(
            (judge.steps.as_slice(), judge.bake_minutes),
            (&[50, 100][..], 5)
        );
        assert!(!judge.automatic_rollback);
        let auto = plan(Lane::AutoMerge, &r, None, Some(&policy), &quiet(), 90);
        assert_eq!(auto.steps, [10, 100]);
    }

    #[test]
    fn a_troubled_area_tightens_the_plan() {
        let areas: AreaHistories = [(
            "billing".to_string(),
            AreaHistory {
                reverts: 1,
                incidents: 1,
                changes: vec!["acme/shop#1".into()],
                ..AreaHistory::default()
            },
        )]
        .into();
        let r = report(&["billing", "orders"]);
        let auto = plan(Lane::AutoMerge, &r, None, None, &areas, 30);
        assert_eq!(auto.steps, [5, 25, 100]);
        assert_eq!(
            auto.reasons[1],
            "`billing` had 1 incident and 1 revert or rollback in the last 30 days: the judge lane's plan instead"
        );
        assert_eq!(auto.lane, Lane::AutoMerge);
        let judge = plan(Lane::Judge, &r, Some("acme/shop#2"), None, &areas, 30);
        assert_eq!(judge.strategy, RolloutStrategy::Flag);
        assert_eq!(judge.steps, [1, 5, 25, 100]);
        assert_eq!(judge.flag.as_deref(), Some("onus-acme-shop-2"));
        assert!(judge.reasons[1].ends_with("the human lane's plan instead"));
        let human = plan(Lane::Human, &r, None, None, &areas, 30);
        assert_eq!(human.bake_minutes, 240);
        assert!(human.reasons[1].ends_with("240 min at each step instead"));
        // An area the change does not touch changes nothing.
        let other = plan(Lane::Judge, &report(&["orders"]), None, None, &areas, 30);
        assert_eq!(other.steps, [5, 25, 100]);
        assert_eq!(other.reasons.len(), 1);
        // Error spikes alone do not tighten.
        let spiky: AreaHistories = [(
            "orders".to_string(),
            AreaHistory {
                error_spikes: 3,
                ..AreaHistory::default()
            },
        )]
        .into();
        assert!(troubled(&spiky, &["orders".into()]).is_empty());
    }

    fn sample(name: &str, value: f64, baseline: Option<f64>) -> MetricSample {
        MetricSample {
            name: name.into(),
            value,
            baseline,
            unit: None,
        }
    }

    #[test]
    fn stages_are_decided_from_their_samples() {
        let mut judge = plan(Lane::Judge, &report(&["orders"]), None, None, &quiet(), 90);
        judge.guards.push(MetricGuard {
            metric: "checkout_conversion".into(),
            max: None,
            min: None,
            max_ratio: None,
            min_ratio: Some(0.97),
        });
        let ok = [
            sample("error_rate", 0.012, Some(0.012)),
            sample("latency_p95_ms", 200.0, Some(190.0)),
            sample("checkout_conversion", 0.05, Some(0.05)),
        ];
        let at = |percent: u8, samples: &[MetricSample], baked: u64| {
            evaluate("acme/shop#42", &judge, percent, samples, 1000, 1000 + baked)
        };
        // Healthy and baked: the next step.
        let d = at(5, &ok, 1800);
        assert_eq!((d.decision, d.next), (Decision::Promote, Some(25)));
        assert_eq!(d.change, "acme/shop#42");
        assert_eq!(at(25, &ok, 1800).next, Some(100));
        // A share between steps goes to the next one.
        assert_eq!(at(10, &ok, 1800).next, Some(25));
        assert_eq!(at(100, &ok, 1800).decision, Decision::Complete);
        // Not baked yet.
        let d = at(5, &ok, 600);
        assert_eq!(d.decision, Decision::Hold);
        assert_eq!(d.reasons, ["baked 10 of 30 min"]);
        // A guard breached.
        let mut errors = ok.clone();
        errors[0] = sample("error_rate", 0.031, Some(0.012));
        let d = at(25, &errors, 1800);
        assert_eq!((d.decision, d.next), (Decision::Rollback, None));
        assert_eq!(
            d.reasons,
            ["error_rate 0.031 is 2.6× its baseline 0.012 (limit 1.5×)"]
        );
        let mut dropped = ok.clone();
        dropped[2] = sample("checkout_conversion", 0.045, Some(0.05));
        let d = at(25, &dropped, 1800);
        assert_eq!(d.decision, Decision::Rollback);
        assert_eq!(
            d.reasons,
            ["checkout_conversion 0.045 is 0.9× its baseline 0.05 (at least 0.97×)"]
        );
        // A missing sample or baseline holds.
        let d = at(25, &ok[..2], 1800);
        assert_eq!(d.decision, Decision::Hold);
        assert_eq!(d.reasons, ["no checkout_conversion sample"]);
        let mut zero = ok.clone();
        zero[1] = sample("latency_p95_ms", 200.0, Some(0.0));
        assert_eq!(at(25, &zero, 1800).decision, Decision::Hold);
        // A breach outranks a hold, baked or not.
        let d = at(25, &errors[..2], 60);
        assert_eq!(d.decision, Decision::Rollback);
        // Absolute limits.
        let mut capped = judge.clone();
        capped.guards = vec![MetricGuard {
            metric: "error_rate".into(),
            max: Some(0.02),
            min: None,
            max_ratio: None,
            min_ratio: None,
        }];
        let d = evaluate("c", &capped, 5, &errors, 0, 0);
        assert_eq!(d.reasons, ["error_rate 0.031 is above 0.02"]);
    }
}
