//! The learned lane model (ADR 0015): a small logistic regression, trained
//! on outcome records, that may raise a change's lane and never lower it.
//!
//! - [`features`]: a fixed, named set of numbers that describe a change when
//!   it is classified: its report, its submission and the history of what it
//!   touches. CI keeps them with the change's records.
//! - [`label`]: whether a recorded change was a problem (an incident, a
//!   revert or rollback, an audit's miss, or `/onus caught`), a clean
//!   landing, or not usable yet.
//! - [`train`]: deterministic. The same records, in any line order, give
//!   the same model file, byte for byte.
//! - [`Model::predict`]: the change's risk and what drove it.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use onus_core::{ChangeKind, Confidence, SemanticChange, SemanticReport, TestReach, ids};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::classify::{Record, area};
use crate::outcomes::{Outcome, repo_of};
use crate::submission::Submission;

/// The features this binary computes. A model trained on another set is
/// stale and stays inert until it is retrained.
pub const FEATURE_SET: u32 = 1;

/// The model file's format.
pub const SCHEMA: u32 = 1;

/// Every feature, in the order the model stores its weights.
pub const FEATURES: &[&str] = &[
    // ln(1 + rows in the report)
    "rows",
    // ln(1 + rows of that kind)
    "kind.additive",
    "kind.breaking",
    "kind.internal",
    "kind.dependency",
    "kind.config",
    "kind.test",
    "kind.security-sensitive",
    // ln(1 + the largest hints.blastRadius)
    "blastRadius",
    // ln(1 + the largest hints.dependentComponents)
    "dependentComponents",
    // ln(1 + lines added and removed)
    "linesChanged",
    // ln(1 + files changed)
    "files",
    // 0/1: a row's novelty (or subkind) says so
    "novelty.new-dependency",
    "novelty.new-package",
    "novelty.new-vendor",
    "novelty.new-data-egress",
    "novelty.new-api",
    "novelty.new-cross-component-dependency",
    // 0/1: a row in labelled (sensitive) code
    "sensitive",
    // ln(1 + rows that need a person)
    "needsPerson",
    // share of rows on parts of the map Onus inferred or could not resolve
    "lowConfidence",
    // share of rows whose code no test exercises and no trace observed
    "untested",
    // ln(1 + rows with subkind contract-changed-unverified)
    "unverifiedContracts",
    // 0/1: a tests-removed-with-code row
    "testsRemoved",
    // ln(1 + new boundary-rule violations)
    "ruleViolations",
    // 0/1: no stated intent
    "noIntent",
    // ln(1 + the submission's test runs that passed)
    "evidencePassed",
    // 0/1: a test run in the submission failed
    "evidenceFailed",
    // ln(1 + earlier changes touching any of this change's components)
    "areaChanges",
    // ln(1 + those that were problems)
    "areaProblems",
    // problems / (changes + 5): shrinks to 0 with little history
    "areaProblemRate",
    // ln(1 + the agent setup's changes on record)
    "agentChanges",
    // ln(1 + incidents among its recent changes)
    "agentIncidents",
    // problems / (changes + 5) over the setup's earlier changes
    "agentProblemRate",
];

const KINDS: [ChangeKind; 7] = [
    ChangeKind::Additive,
    ChangeKind::Breaking,
    ChangeKind::Internal,
    ChangeKind::Dependency,
    ChangeKind::Config,
    ChangeKind::Test,
    ChangeKind::SecuritySensitive,
];

const NOVELTY: [&str; 6] = [
    "new-dependency",
    "new-package",
    "new-vendor",
    "new-data-egress",
    "new-api",
    "new-cross-component-dependency",
];

/// A rate is problems / (changes + PRIOR), so a little history says little.
const PRIOR: f64 = 5.0;

/// The L2 penalty on the weights (the bias is not penalised).
pub const L2: f64 = 0.01;
/// Gradient descent steps, from zero.
pub const ITERATIONS: u32 = 3000;
/// The largest step size; a smaller one is used when the features are many
/// and alike, so the fit cannot overshoot.
pub const LEARNING_RATE: f64 = 0.5;

fn round_to(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

fn round6(x: f64) -> f64 {
    round_to(x, 6)
}

fn ln1(n: f64) -> f64 {
    (1.0 + n).ln()
}

fn flag(b: bool) -> f64 {
    f64::from(u8::from(b))
}

fn sigmoid(s: f64) -> f64 {
    if s >= 0.0 {
        1.0 / (1.0 + (-s).exp())
    } else {
        let e = s.exp();
        e / (1.0 + e)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What a change looked like when it was classified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    /// [`FEATURE_SET`] when it was computed; a model ignores other sets.
    pub set: u32,
    /// The change's components (`root` for code outside every component),
    /// sorted: the area later changes compare against.
    pub components: Vec<String>,
    /// One value per name in [`FEATURES`], rounded to 1e-6.
    pub values: BTreeMap<String, f64>,
}

impl Features {
    /// A feature's value; 0 when it is missing.
    pub fn get(&self, name: &str) -> f64 {
        self.values.get(name).copied().unwrap_or(0.0)
    }

    fn vector(&self) -> Vec<f64> {
        FEATURES.iter().map(|n| self.get(n)).collect()
    }
}

/// What the history features look at.
#[derive(Debug, Clone, Copy, Default)]
pub struct History<'a> {
    pub outcomes: &'a [Outcome],
    /// The agent setup key (`tool/model/config`), when there is a submission.
    pub agent: Option<&'a str>,
    pub record: Option<Record>,
    /// `owner/repo`: only this repository's records count (records files can
    /// hold several repositories). Records of a bare commit count for all.
    pub repo: Option<&'a str>,
    /// The change being classified, left out of its own history.
    pub change: Option<&'a str>,
    /// Unix seconds; later records do not count, so a backfill sees the
    /// history as it was then.
    pub until: Option<u64>,
}

/// A record that makes its change a problem: an incident, a revert or
/// rollback (not a planned one), an audit's miss, or a person's `/onus
/// caught`.
fn problem(o: &Outcome) -> bool {
    let bad = match o.result.as_str() {
        "incident" | "reverted" => true,
        "rolled-back" => !o.planned,
        _ => false,
    };
    bad || o.missed || o.caught
}

/// One earlier change, from all its records.
struct Earlier<'o> {
    agent: &'o str,
    areas: BTreeSet<&'o str>,
    problem: bool,
}

fn earlier<'o>(h: &History<'o>) -> BTreeMap<&'o str, Earlier<'o>> {
    let mut out: BTreeMap<&str, Earlier> = BTreeMap::new();
    for o in h.outcomes {
        if h.until.is_some_and(|u| o.at > u) || h.change == Some(o.change.as_str()) {
            continue;
        }
        if let (Some(want), Some(repo)) = (h.repo, repo_of(&o.change))
            && want != repo
        {
            continue;
        }
        let e = out.entry(&o.change).or_insert_with(|| Earlier {
            agent: &o.agent,
            areas: BTreeSet::new(),
            problem: false,
        });
        e.areas.extend(
            o.components
                .iter()
                .map(String::as_str)
                .chain(o.involved.iter().map(|i| ids::component_of(i).unwrap_or(i))),
        );
        e.problem |= problem(o);
    }
    out
}

/// Whether a novelty entry (`new-vendor:Acme SMS`) is of kind `kind`.
fn novel(entry: &str, kind: &str) -> bool {
    entry
        .strip_prefix(kind)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

/// The change's features: its report, its submission if any, and the
/// history of the components it touches and of its agent setup.
pub fn features(
    report: &SemanticReport,
    submission: Option<&Submission>,
    history: &History,
) -> Features {
    let rows = &report.changes;
    let count = |f: &dyn Fn(&SemanticChange) -> bool| rows.iter().filter(|r| f(r)).count() as f64;
    let any = |f: &dyn Fn(&SemanticChange) -> bool| flag(rows.iter().any(f));
    let share = |k: f64| {
        if rows.is_empty() {
            0.0
        } else {
            k / rows.len() as f64
        }
    };
    let mut components: Vec<String> = rows.iter().filter_map(area).map(str::to_string).collect();
    components.sort();
    components.dedup();
    let mut values = BTreeMap::new();
    let mut put = |name: &str, value: f64| {
        values.insert(name.to_string(), round6(value));
    };
    put("rows", ln1(rows.len() as f64));
    for k in KINDS {
        put(
            &format!("kind.{}", k.as_str()),
            ln1(count(&|r| r.kind == k)),
        );
    }
    let max = |f: &dyn Fn(&SemanticChange) -> u32| f64::from(rows.iter().map(f).max().unwrap_or(0));
    put("blastRadius", ln1(max(&|r| r.hints.blast_radius)));
    put(
        "dependentComponents",
        ln1(max(&|r| r.hints.dependent_components)),
    );
    let text = &report.text_stats;
    put(
        "linesChanged",
        ln1(f64::from(text.lines_added) + f64::from(text.lines_removed)),
    );
    put("files", ln1(f64::from(text.files)));
    for kind in NOVELTY {
        put(
            &format!("novelty.{kind}"),
            any(&|r| r.subkind == kind || r.hints.novelty.iter().any(|n| novel(n, kind))),
        );
    }
    put("sensitive", any(&|r| !r.hints.labels.is_empty()));
    put("needsPerson", ln1(count(&|r| r.hints.needs_person)));
    put(
        "lowConfidence",
        share(count(&|r| {
            matches!(r.hints.confidence, Confidence::Low | Confidence::Inferred)
        })),
    );
    put(
        "untested",
        share(count(&|r| r.hints.tested == Some(TestReach::Untested))),
    );
    put(
        "unverifiedContracts",
        ln1(count(&|r| r.subkind == "contract-changed-unverified")),
    );
    put(
        "testsRemoved",
        any(&|r| r.subkind == "tests-removed-with-code"),
    );
    put(
        "ruleViolations",
        ln1(f64::from(report.summary.new_rule_violations)),
    );
    put("noIntent", flag(report.intent_check.is_none()));
    let evidence = submission
        .map(|s| s.evidence.as_slice())
        .unwrap_or_default();
    put(
        "evidencePassed",
        ln1(evidence.iter().filter(|r| !r.failed()).count() as f64),
    );
    put("evidenceFailed", flag(evidence.iter().any(|r| r.failed())));
    let earlier = earlier(history);
    let touched: BTreeSet<&str> = components.iter().map(String::as_str).collect();
    let rate = |changes: &[&Earlier]| {
        let problems = changes.iter().filter(|e| e.problem).count() as f64;
        (problems, problems / (changes.len() as f64 + PRIOR))
    };
    let in_area: Vec<&Earlier> = earlier
        .values()
        .filter(|e| !e.areas.is_disjoint(&touched))
        .collect();
    let (problems, area_rate) = rate(&in_area);
    put("areaChanges", ln1(in_area.len() as f64));
    put("areaProblems", ln1(problems));
    put("areaProblemRate", area_rate);
    let record = history.record.unwrap_or_default();
    put("agentChanges", ln1(f64::from(record.changes)));
    put("agentIncidents", ln1(f64::from(record.recent_incidents)));
    let mine: Vec<&Earlier> = match history.agent {
        Some(agent) => earlier.values().filter(|e| e.agent == agent).collect(),
        None => Vec::new(),
    };
    put("agentProblemRate", rate(&mine).1);
    debug_assert_eq!(values.len(), FEATURES.len());
    Features {
        set: FEATURE_SET,
        components,
        values,
    }
}

/// Whether a recorded change was a problem: `Some(true)` a problem,
/// `Some(false)` a clean landing, `None` not usable yet. `o` is the change's
/// latest record; `had_problem` says whether any of its records was a
/// problem (an incident, a revert or rollback, an audit's miss, or a
/// person's `/onus caught`). A merge is clean only once it is `settle_secs`
/// old at `through`, so reverts and incidents have had time to show.
pub fn label(o: &Outcome, had_problem: bool, through: u64, settle_secs: u64) -> Option<bool> {
    if o.features.as_ref()?.set != FEATURE_SET {
        return None;
    }
    if had_problem || problem(o) {
        return Some(true);
    }
    // A planned rollback (a flag turned off on purpose) is no problem.
    let merged = o.result == "merged" || (o.result == "rolled-back" && o.planned);
    (merged && o.at.saturating_add(settle_secs) <= through).then_some(false)
}

/// One labelled change.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub change: String,
    /// Its latest record's time.
    pub at: u64,
    pub problem: bool,
    /// Its features, in the order of [`FEATURES`].
    pub values: Vec<f64>,
}

/// Every change's latest record, by time and then by its JSON, so the
/// choice does not depend on line order.
fn latest_by_time(all: &[Outcome]) -> Vec<&Outcome> {
    let mut last: BTreeMap<&str, (&Outcome, String)> = BTreeMap::new();
    for o in all {
        let line = serde_json::to_string(o).unwrap_or_default();
        match last.get(o.change.as_str()) {
            Some((kept, kept_line)) if (kept.at, kept_line) >= (o.at, &line) => {}
            _ => {
                last.insert(&o.change, (o, line));
            }
        }
    }
    last.into_values().map(|(o, _)| o).collect()
}

/// The labelled changes, sorted by change, and how many were skipped and
/// why (`noFeatures`, `staleFeatures`, `unsettled`, `undecided`). The time
/// they are measured at is the latest record's, never the clock.
pub fn examples(all: &[Outcome], settle_secs: u64) -> (Vec<Example>, BTreeMap<String, u32>) {
    let through = all.iter().map(|o| o.at).max().unwrap_or(0);
    let problems: BTreeSet<&str> = all
        .iter()
        .filter(|o| problem(o))
        .map(|o| o.change.as_str())
        .collect();
    let mut skipped: BTreeMap<String, u32> =
        ["noFeatures", "staleFeatures", "unsettled", "undecided"]
            .map(|k| (k.to_string(), 0))
            .into();
    let mut skip = |why: &str| *skipped.entry(why.to_string()).or_default() += 1;
    let mut out = Vec::new();
    for o in latest_by_time(all) {
        let Some(f) = &o.features else {
            skip("noFeatures");
            continue;
        };
        if f.set != FEATURE_SET {
            skip("staleFeatures");
            continue;
        }
        match label(
            o,
            problems.contains(o.change.as_str()),
            through,
            settle_secs,
        ) {
            Some(problem) => out.push(Example {
                change: o.change.clone(),
                at: o.at,
                problem,
                values: f.vector(),
            }),
            None if o.result == "merged" => skip("unsettled"),
            None => skip("undecided"),
        }
    }
    out.sort_by(|a, b| a.change.cmp(&b.change));
    (out, skipped)
}

/// How a training run went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Training {
    pub examples: u32,
    /// Examples that were problems.
    pub positives: u32,
    /// Changes left out, and why: `noFeatures`, `staleFeatures`,
    /// `unsettled`, `undecided`.
    pub skipped: BTreeMap<String, u32>,
    pub settle_days: u32,
    /// The latest record time among the examples (Unix seconds).
    pub through: u64,
    /// The sha256 of the sorted examples: what the model was trained on.
    pub outcomes_sha256: String,
    pub l2: f64,
    pub iterations: u32,
    /// The step size used.
    pub learning_rate: f64,
}

/// At a threshold: the share of held-out changes at or above it, and the
/// share of held-out problems at or above it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub raised: Option<f64>,
    pub recall: Option<f64>,
}

/// How a model fitted without the held-out fifth did on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holdout {
    pub examples: u32,
    pub positives: u32,
    /// The area under the ROC curve; absent without both classes.
    pub auc: Option<f64>,
    pub at_judge: Rates,
    pub at_human: Rates,
}

/// The model file (`onus lanes train`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub schema: u32,
    /// The feature set it was trained on.
    pub feature_set: u32,
    /// The features, in the order of the weights.
    pub features: Vec<String>,
    pub mean: Vec<f64>,
    /// The standard deviation; 1 where a feature never varied.
    pub scale: Vec<f64>,
    pub weights: Vec<f64>,
    pub bias: f64,
    pub training: Training,
    pub holdout: Holdout,
}

#[derive(Debug, Clone)]
pub struct TrainOptions {
    pub settle_days: u32,
    pub min_examples: u32,
    pub min_positives: u32,
    /// The thresholds the holdout table is measured at.
    pub judge_at: f64,
    pub human_at: f64,
}

impl Default for TrainOptions {
    fn default() -> Self {
        TrainOptions {
            settle_days: 14,
            min_examples: 50,
            min_positives: 10,
            judge_at: 0.6,
            human_at: 0.85,
        }
    }
}

/// A risk score from standardized features.
struct Fit {
    mean: Vec<f64>,
    scale: Vec<f64>,
    weights: Vec<f64>,
    bias: f64,
    learning_rate: f64,
}

fn logit(mean: &[f64], scale: &[f64], weights: &[f64], bias: f64, x: &[f64]) -> f64 {
    bias + (0..weights.len())
        .map(|j| weights[j] * (x[j] - mean[j]) / scale[j])
        .sum::<f64>()
}

/// Class-balanced, L2-regularised logistic regression, by full-batch
/// gradient descent from zero, summing in example order. Both classes must
/// be present.
fn fit(examples: &[Example]) -> Fit {
    let d = FEATURES.len();
    let n = examples.len() as f64;
    let mean: Vec<f64> = (0..d)
        .map(|j| round6(examples.iter().map(|e| e.values[j]).sum::<f64>() / n))
        .collect();
    let scale: Vec<f64> = (0..d)
        .map(|j| {
            let var = examples
                .iter()
                .map(|e| (e.values[j] - mean[j]).powi(2))
                .sum::<f64>()
                / n;
            let sd = round6(var.sqrt());
            if sd < 1e-9 { 1.0 } else { sd }
        })
        .collect();
    let z: Vec<Vec<f64>> = examples
        .iter()
        .map(|e| (0..d).map(|j| (e.values[j] - mean[j]) / scale[j]).collect())
        .collect();
    let positives = examples.iter().filter(|e| e.problem).count() as f64;
    let negatives = n - positives;
    let w: Vec<f64> = examples
        .iter()
        .map(|e| {
            if e.problem {
                n / (2.0 * positives)
            } else {
                n / (2.0 * negatives)
            }
        })
        .collect();
    let total: f64 = w.iter().sum();
    // The loss is at most this curved (the trace bounds the largest
    // eigenvalue), so a step of 1/curvature never overshoots.
    let trace = 1.0
        + z.iter()
            .zip(&w)
            .map(|(zi, wi)| wi * zi.iter().map(|v| v * v).sum::<f64>())
            .sum::<f64>()
            / total;
    let learning_rate = round6(LEARNING_RATE.min(1.0 / (0.25 * trace + L2)));
    let mut beta = vec![0.0; d];
    let mut bias = 0.0;
    let mut grad = vec![0.0; d];
    for _ in 0..ITERATIONS {
        let mut grad_bias = 0.0;
        grad.iter_mut().for_each(|g| *g = 0.0);
        for (i, e) in examples.iter().enumerate() {
            let s = bias + beta.iter().zip(&z[i]).map(|(b, v)| b * v).sum::<f64>();
            let err = w[i] * (sigmoid(s) - flag(e.problem));
            grad_bias += err;
            for (g, v) in grad.iter_mut().zip(&z[i]) {
                *g += err * v;
            }
        }
        bias -= learning_rate * grad_bias / total;
        for (b, g) in beta.iter_mut().zip(&grad) {
            *b -= learning_rate * (g / total + L2 * *b);
        }
    }
    Fit {
        mean,
        scale,
        weights: beta.into_iter().map(round6).collect(),
        bias: round6(bias),
        learning_rate,
    }
}

impl Fit {
    fn risk(&self, x: &[f64]) -> f64 {
        round6(sigmoid(logit(
            &self.mean,
            &self.scale,
            &self.weights,
            self.bias,
            x,
        )))
    }
}

/// Whether an example is in the held-out fifth: a hash of its change, so
/// the split is the same on every machine.
fn held_out(change: &str) -> bool {
    Sha256::digest(format!("onus holdout {change}").as_bytes())[0] % 5 == 0
}

/// The rank-based area under the ROC curve, ties averaged; `None` without
/// both classes.
fn auc(scored: &[(f64, bool)]) -> Option<f64> {
    let positives = scored.iter().filter(|s| s.1).count() as f64;
    let negatives = scored.len() as f64 - positives;
    if positives == 0.0 || negatives == 0.0 {
        return None;
    }
    let mut order: Vec<usize> = (0..scored.len()).collect();
    order.sort_by(|a, b| scored[*a].0.total_cmp(&scored[*b].0));
    let mut ranks = vec![0.0; scored.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && scored[order[j + 1]].0 == scored[order[i]].0 {
            j += 1;
        }
        // Ranks i+1 ..= j+1, averaged.
        let rank = (i + j) as f64 / 2.0 + 1.0;
        for k in &order[i..=j] {
            ranks[*k] = rank;
        }
        i = j + 1;
    }
    let sum: f64 = scored
        .iter()
        .zip(&ranks)
        .filter(|(s, _)| s.1)
        .map(|(_, r)| r)
        .sum();
    Some((sum - positives * (positives + 1.0) / 2.0) / (positives * negatives))
}

fn rates(scored: &[(f64, bool)], at: f64) -> Rates {
    let share = |of: &[&(f64, bool)]| {
        (!of.is_empty()).then(|| {
            round_to(
                of.iter().filter(|s| s.0 >= at).count() as f64 / of.len() as f64,
                3,
            )
        })
    };
    let all: Vec<&(f64, bool)> = scored.iter().collect();
    let problems: Vec<&(f64, bool)> = scored.iter().filter(|s| s.1).collect();
    Rates {
        raised: share(&all),
        recall: share(&problems),
    }
}

/// Fits the model to `examples` (from [`examples`], with what it skipped).
/// A fifth is held out to measure a first fit on, then the model is fitted
/// again on everything. Fails with "too little data" below the minimums,
/// or when the part left after holding out the fifth lacks either class.
pub fn train(
    examples: &[Example],
    skipped: BTreeMap<String, u32>,
    opts: &TrainOptions,
) -> Result<Model> {
    let mut all = examples.to_vec();
    all.sort_by(|a, b| a.change.cmp(&b.change));
    let n = all.len() as u32;
    let positives = all.iter().filter(|e| e.problem).count() as u32;
    if n < opts.min_examples {
        bail!(
            "too little data: {n} labelled changes, at least {} are needed (--min-examples)",
            opts.min_examples
        );
    }
    if positives < opts.min_positives {
        bail!(
            "too little data: {positives} problems among {n} labelled changes, at least {} are needed (--min-positives)",
            opts.min_positives
        );
    }
    let (held, rest): (Vec<Example>, Vec<Example>) =
        all.iter().cloned().partition(|e| held_out(&e.change));
    let rest_positives = rest.iter().filter(|e| e.problem).count();
    if rest_positives == 0 || rest_positives == rest.len() {
        bail!(
            "too little data: after holding out a fifth to measure on, {} of the other {} changes are problems; both problems and clean merges are needed",
            rest_positives,
            rest.len()
        );
    }
    let first = fit(&rest);
    let scored: Vec<(f64, bool)> = held
        .iter()
        .map(|e| (first.risk(&e.values), e.problem))
        .collect();
    let holdout = Holdout {
        examples: held.len() as u32,
        positives: held.iter().filter(|e| e.problem).count() as u32,
        auc: auc(&scored).map(|a| round_to(a, 3)),
        at_judge: rates(&scored, opts.judge_at),
        at_human: rates(&scored, opts.human_at),
    };
    let model = fit(&all);
    Ok(Model {
        schema: SCHEMA,
        feature_set: FEATURE_SET,
        features: FEATURES.iter().map(|f| f.to_string()).collect(),
        mean: model.mean,
        scale: model.scale,
        weights: model.weights,
        bias: model.bias,
        training: Training {
            examples: n,
            positives,
            skipped,
            settle_days: opts.settle_days,
            through: all.iter().map(|e| e.at).max().unwrap_or(0),
            outcomes_sha256: hex(&Sha256::digest(serde_json::to_vec(&all)?)),
            l2: L2,
            iterations: ITERATIONS,
            learning_rate: model.learning_rate,
        },
        holdout,
    })
}

/// A model's view of a change.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    /// 0 to 1, rounded to 1e-6. Class-balanced, so a relative risk score,
    /// not a calibrated probability.
    pub risk: f64,
    pub logit: f64,
    /// Each feature's index and its contribution to the logit.
    pub contributions: Vec<(usize, f64)>,
}

/// A feature that pushed the risk up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Factor {
    pub feature: String,
    /// The feature's value for this change.
    pub value: f64,
    /// What it added to the logit.
    pub contribution: f64,
    /// In plain words: "2 breaking rows".
    pub text: String,
}

impl Model {
    /// Whether this binary can apply the model: its schema, feature set and
    /// features match. A model that cannot is stale.
    pub fn usable(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!(
                "the model file has schema {}, and this onus reads schema {SCHEMA}",
                self.schema
            ));
        }
        if self.feature_set != FEATURE_SET || self.features != FEATURES {
            return Err(format!(
                "the model was trained on feature set {} and this onus computes set {FEATURE_SET}",
                self.feature_set
            ));
        }
        let d = FEATURES.len();
        if [self.mean.len(), self.scale.len(), self.weights.len()] != [d, d, d] {
            return Err(format!("the model does not have {d} weights"));
        }
        let finite = self
            .mean
            .iter()
            .chain(&self.scale)
            .chain(&self.weights)
            .chain([&self.bias])
            .all(|v| v.is_finite());
        if !finite || self.scale.contains(&0.0) {
            return Err("the model holds a value that is not a number".into());
        }
        Ok(())
    }

    /// The change's risk and what each feature added. Expects a usable model.
    pub fn predict(&self, f: &Features) -> Prediction {
        let contributions: Vec<(usize, f64)> = self
            .features
            .iter()
            .enumerate()
            .map(|(j, name)| {
                (
                    j,
                    self.weights[j] * (f.get(name) - self.mean[j]) / self.scale[j],
                )
            })
            .collect();
        let logit = self.bias + contributions.iter().map(|c| c.1).sum::<f64>();
        Prediction {
            risk: round6(sigmoid(logit)),
            logit,
            contributions,
        }
    }

    /// The three features that pushed the risk up most, largest first.
    pub fn factors(&self, p: &Prediction, f: &Features) -> Vec<Factor> {
        let mut up: Vec<(usize, f64)> = p
            .contributions
            .iter()
            .copied()
            .filter(|c| c.1 > 0.0)
            .collect();
        up.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        up.into_iter()
            .take(3)
            .map(|(j, c)| {
                let name = &self.features[j];
                let value = f.get(name);
                Factor {
                    feature: name.clone(),
                    value,
                    contribution: round_to(c, 3),
                    text: describe(name, value, &f.components),
                }
            })
            .collect()
    }
}

/// The first 12 hex digits of the model file's sha256: how reasons name it.
pub fn model_id(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))[..12].to_string()
}

/// A feature's value in plain words: `kind.breaking`, ln 3 → "2 breaking
/// rows"; `areaProblems` → "3 earlier problems in billing".
pub fn describe(feature: &str, value: f64, components: &[String]) -> String {
    // Back from ln(1 + n).
    let n = (value.exp() - 1.0).round().max(0.0) as u64;
    let s = |one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let pct = format!("{:.0}%", value * 100.0);
    let areas = match components {
        [] => "the code it touches".to_string(),
        [one] => one.clone(),
        [rest @ .., last] if rest.len() < 3 => format!("{} and {last}", rest.join(", ")),
        _ => format!(
            "{} and {} more",
            components[..3].join(", "),
            components.len() - 3
        ),
    };
    match feature {
        "rows" => s("row", "rows"),
        "blastRadius" => format!("blast radius {n}"),
        "dependentComponents" => s("dependent component", "dependent components"),
        "linesChanged" => s("line changed", "lines changed"),
        "files" => s("file changed", "files changed"),
        "novelty.new-dependency" => "a new dependency".into(),
        "novelty.new-package" => "third-party code new to the repository".into(),
        "novelty.new-vendor" => "a new vendor".into(),
        "novelty.new-data-egress" => "data sent somewhere new".into(),
        "novelty.new-api" => "a new external API".into(),
        "novelty.new-cross-component-dependency" => "a new dependency between components".into(),
        "sensitive" => "sensitive code".into(),
        "needsPerson" => s("row that needs a person", "rows that need a person"),
        "lowConfidence" => format!("{pct} of rows on parts of the map Onus could not resolve"),
        "untested" => format!("{pct} of rows untested"),
        "unverifiedContracts" => s("unverified contract change", "unverified contract changes"),
        "testsRemoved" => "tests removed with code".into(),
        "ruleViolations" => s("new rule violation", "new rule violations"),
        "noIntent" => "no stated intent".into(),
        "evidencePassed" => s("passing test run", "passing test runs"),
        "evidenceFailed" => "a failing test run".into(),
        "areaChanges" => format!("{} in {areas}", s("earlier change", "earlier changes")),
        "areaProblems" => format!("{} in {areas}", s("earlier problem", "earlier problems")),
        "areaProblemRate" => format!("a problem rate of {value:.2} in {areas}"),
        "agentChanges" => s(
            "change on record for the agent setup",
            "changes on record for the agent setup",
        ),
        "agentIncidents" => s(
            "recent incident by the agent setup",
            "recent incidents by the agent setup",
        ),
        "agentProblemRate" => format!("a problem rate of {value:.2} for the agent setup"),
        kind => match kind.strip_prefix("kind.") {
            Some(k) => s(&format!("{k} row"), &format!("{k} rows")),
            None => format!("{kind} {value}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{ChangeLevel, Hints, IntentCheck, Lane, ReportSummary, StructureNotes};
    use onus_doors::runner::TestRun;

    use crate::submission::AgentSetup;

    fn row(kind: ChangeKind, subkind: &str, component: &str) -> SemanticChange {
        SemanticChange {
            id: format!("{subkind}:{component}"),
            kind,
            subkind: subkind.into(),
            level: ChangeLevel::Structure,
            subject: "x".into(),
            component: Some(component.into()),
            kind_label: String::new(),
            title: subkind.into(),
            why_it_matters: String::new(),
            hints: Hints {
                labels: vec![],
                blast_radius: 0,
                novelty: vec![],
                confidence: Confidence::Static,
                rules_of_the_game: false,
                intent_mismatch: false,
                needs_person: false,
                dependent_components: 0,
                dependents: vec![],
                tested: None,
                owners: vec![],
            },
            locations: vec![],
            stats: None,
        }
    }

    fn report(rows: Vec<SemanticChange>) -> SemanticReport {
        SemanticReport {
            schema_version: 1,
            base: "base".into(),
            head: "head".into(),
            summary: ReportSummary {
                meaning_changes: rows.len() as u32,
                needs_attention: 0,
                secrets: 0,
                new_rule_violations: 0,
                intent_mismatches: 0,
            },
            changes: rows,
            intent_check: None,
            rule_violations: vec![],
            structure: StructureNotes::default(),
            text_stats: Default::default(),
            map_diagnostics: vec![],
        }
    }

    fn outcome(change: &str, result: &str, at: u64, components: &[&str]) -> Outcome {
        Outcome {
            at,
            change: change.into(),
            agent: "a/b/c".into(),
            judge: None,
            lane: Lane::Judge,
            verdict: None,
            result: result.into(),
            audited: false,
            missed: false,
            commit: None,
            involved: vec![],
            note: None,
            components: components.iter().map(|c| c.to_string()).collect(),
            reviewed_by: vec![],
            sampled: false,
            audit: None,
            merged_by: None,
            features: None,
            caught: false,
            signal: None,
            planned: false,
        }
    }

    fn snapshot(values: &[(&str, f64)]) -> Features {
        let mut f = features(&report(vec![]), None, &History::default());
        for (k, v) in values {
            f.values.insert(k.to_string(), *v);
        }
        f
    }

    #[test]
    fn features_have_every_name_in_order() {
        let mut breaking = row(ChangeKind::Breaking, "export-removed", "billing");
        breaking.hints.novelty = vec!["new-vendor:Acme SMS".into()];
        breaking.hints.confidence = Confidence::Low;
        let mut r = report(vec![
            breaking,
            row(ChangeKind::Breaking, "export-removed", "billing"),
        ]);
        r.intent_check = Some(IntentCheck {
            stated: "x".into(),
            touches: vec![],
            contracts: vec![],
            externals: vec![],
            mismatches: vec![],
        });
        let run = |exit_code| TestRun {
            commit: "h".into(),
            image: "node".into(),
            setup: None,
            command: "npm test".into(),
            exit_code,
            output_tail: String::new(),
            manifest: None,
        };
        let sub = Submission {
            schema: 1,
            base: "b".into(),
            head: "h".into(),
            intent: None,
            report: r.clone(),
            changed_files: vec![],
            evidence: vec![run(0), run(1)],
            scope: None,
            escalations: vec![],
            approvals: vec![],
            agent: AgentSetup::default(),
        };
        let f = features(&r, Some(&sub), &History::default());
        assert_eq!(f.set, FEATURE_SET);
        assert_eq!(f.components, ["billing"]);
        let names: BTreeSet<&str> = f.values.keys().map(String::as_str).collect();
        assert_eq!(names, FEATURES.iter().copied().collect());
        assert_eq!(FEATURES.len(), names.len(), "no name twice");
        assert_eq!(f.get("kind.breaking"), round6(3f64.ln()));
        assert_eq!(f.get("kind.internal"), 0.0);
        assert_eq!(f.get("novelty.new-vendor"), 1.0);
        assert_eq!(f.get("novelty.new-api"), 0.0);
        assert_eq!(f.get("lowConfidence"), 0.5);
        assert_eq!(f.get("evidenceFailed"), 1.0);
        assert_eq!(f.get("evidencePassed"), round6(2f64.ln()));
        assert_eq!(f.get("noIntent"), 0.0);
        // Without a submission, no evidence.
        let bare = features(&r, None, &History::default());
        assert_eq!(
            (bare.get("evidenceFailed"), bare.get("evidencePassed")),
            (0.0, 0.0)
        );
    }

    #[test]
    fn area_history_is_this_repos_and_leaves_out_the_change() {
        let r = report(vec![row(
            ChangeKind::Internal,
            "internal-changes",
            "billing",
        )]);
        let all = vec![
            outcome("acme/shop#1", "merged", 10, &["billing"]),
            outcome("acme/shop#1", "incident", 20, &["billing"]),
            outcome("acme/other#2", "incident", 20, &["billing"]),
            // The change itself.
            outcome("acme/shop#9", "incident", 20, &["billing"]),
            Outcome {
                involved: vec!["billing:src/a.ts#charge".into()],
                ..outcome("acme/shop#3", "incident", 20, &[])
            },
            outcome("acme/shop#4", "merged", 20, &["billing"]),
            outcome("acme/shop#5", "incident", 20, &["orders"]),
            // After the change was classified.
            outcome("acme/shop#6", "incident", 99, &["billing"]),
        ];
        let f = features(
            &r,
            None,
            &History {
                outcomes: &all,
                repo: Some("acme/shop"),
                change: Some("acme/shop#9"),
                until: Some(50),
                ..History::default()
            },
        );
        assert_eq!(f.get("areaChanges"), round6(4f64.ln()));
        assert_eq!(f.get("areaProblems"), round6(3f64.ln()));
        assert_eq!(f.get("areaProblemRate"), round6(2.0 / 8.0));
        let factor = describe("areaProblems", f.get("areaProblems"), &f.components);
        assert_eq!(factor, "2 earlier problems in billing");
    }

    #[test]
    fn labels() {
        let with = |result: &str, at: u64| Outcome {
            features: Some(snapshot(&[])),
            ..outcome("acme/shop#1", result, at, &[])
        };
        let day = 86_400;
        let label_of = |o: &Outcome, had: bool| label(o, had, 30 * day, 14 * day);
        assert_eq!(label_of(&with("merged", day), true), Some(true));
        for result in ["incident", "reverted", "rolled-back"] {
            assert_eq!(label_of(&with(result, 29 * day), false), Some(true));
        }
        let planned = Outcome {
            planned: true,
            ..with("rolled-back", day)
        };
        assert_eq!(label_of(&planned, false), Some(false));
        let missed = Outcome {
            missed: true,
            ..with("merged", 29 * day)
        };
        assert_eq!(label_of(&missed, false), Some(true));
        let caught = Outcome {
            caught: true,
            ..with("closed", 29 * day)
        };
        assert_eq!(label_of(&caught, false), Some(true));
        assert_eq!(label_of(&with("merged", day), false), Some(false));
        assert_eq!(label_of(&with("merged", 20 * day), false), None);
        assert_eq!(label_of(&with("open", day), false), None);
        assert_eq!(label_of(&with("closed", day), false), None);
        assert_eq!(
            label_of(&outcome("acme/shop#1", "incident", day, &[]), true),
            None
        );
        let mut stale = with("incident", day);
        stale.features.as_mut().unwrap().set = 0;
        assert_eq!(label_of(&stale, true), None);
    }

    /// `n` changes. Every third has a breaking row and is a problem unless
    /// `i % 13 == 0`; the others are clean unless `i % 17 == 0`. The number
    /// of internal rows and the size are noise.
    fn synthetic(n: usize) -> Vec<Outcome> {
        let day = 86_400;
        (0..n)
            .map(|i| {
                let breaking = i % 3 == 0;
                let problem = if breaking { i % 13 != 0 } else { i % 17 == 0 };
                let mut rows: Vec<SemanticChange> = (0..=i % 4)
                    .map(|_| row(ChangeKind::Internal, "internal-changes", "billing"))
                    .collect();
                if breaking {
                    rows.push(row(ChangeKind::Breaking, "export-removed", "billing"));
                }
                let mut r = report(rows);
                r.text_stats.lines_added = (i * 37 % 200) as u32;
                r.text_stats.files = (i % 5) as u32 + 1;
                let f = features(&r, None, &History::default());
                let result = if problem { "reverted" } else { "merged" };
                Outcome {
                    features: Some(f),
                    ..outcome(&format!("acme/shop#{i}"), result, i as u64 * 60, &[])
                }
            })
            .chain([outcome("acme/shop#late", "open", 100 * day, &[])])
            .collect()
    }

    fn quick() -> TrainOptions {
        TrainOptions {
            settle_days: 14,
            ..TrainOptions::default()
        }
    }

    fn model_of(all: &[Outcome]) -> Model {
        let (ex, skipped) = examples(all, 14 * 86_400);
        train(&ex, skipped, &quick()).unwrap()
    }

    #[test]
    fn training_is_deterministic_and_order_free() {
        let all = synthetic(90);
        let mut shuffled = all.clone();
        shuffled.reverse();
        shuffled.sort_by_key(|o| model_id(o.change.as_bytes()));
        let a = serde_json::to_string_pretty(&model_of(&all)).unwrap();
        let b = serde_json::to_string_pretty(&model_of(&shuffled)).unwrap();
        assert_eq!(a, b);
        let m = model_of(&all);
        assert_eq!(m.training.examples, 90);
        assert_eq!(m.training.skipped["noFeatures"], 1);
        assert_eq!(m.training.through, 89 * 60);
    }

    #[test]
    fn training_learns_a_signal() {
        let m = model_of(&synthetic(200));
        let breaking = FEATURES.iter().position(|f| *f == "kind.breaking").unwrap();
        let top = (0..m.weights.len())
            .max_by(|a, b| m.weights[*a].total_cmp(&m.weights[*b]))
            .unwrap();
        assert_eq!(top, breaking, "{:?}", m.weights);
        assert!(m.holdout.auc.unwrap() > 0.85, "{:?}", m.holdout);
        let risk = |rows| {
            m.predict(&features(&report(rows), None, &History::default()))
                .risk
        };
        let internal = || row(ChangeKind::Internal, "internal-changes", "billing");
        let with_breaking = risk(vec![
            internal(),
            row(ChangeKind::Breaking, "export-removed", "billing"),
        ]);
        assert!(with_breaking > risk(vec![internal()]));
    }

    #[test]
    fn train_refuses_too_little_data() {
        let all = synthetic(200);
        let (ex, skipped) = examples(&all, 14 * 86_400);
        let err = train(&ex[..49], skipped.clone(), &quick()).unwrap_err();
        assert!(
            err.to_string()
                .contains("too little data: 49 labelled changes, at least 50 are needed"),
            "{err}"
        );
        let few: Vec<Example> = ex
            .iter()
            .filter(|e| !e.problem)
            .cloned()
            .chain(ex.iter().filter(|e| e.problem).take(9).cloned())
            .collect();
        let err = train(&few, skipped.clone(), &quick()).unwrap_err();
        assert!(
            err.to_string().contains(&format!(
                "9 problems among {} labelled changes, at least 10 are needed",
                few.len()
            )),
            "{err}"
        );
        // Every problem held out: nothing left to learn problems from.
        let held: Vec<Example> = ex
            .iter()
            .filter(|e| !e.problem || held_out(&e.change))
            .cloned()
            .collect();
        let opts = TrainOptions {
            min_positives: 1,
            ..quick()
        };
        let err = train(&held, skipped, &opts).unwrap_err();
        assert!(
            err.to_string().contains("after holding out a fifth"),
            "{err}"
        );
    }

    #[test]
    fn predict_explains_itself() {
        let m = model_of(&synthetic(200));
        let mut r = report(vec![
            row(ChangeKind::Breaking, "export-removed", "billing"),
            row(ChangeKind::Breaking, "export-removed", "orders"),
        ]);
        r.text_stats.lines_added = 400;
        let f = features(&r, None, &History::default());
        let p = m.predict(&f);
        let sum: f64 = p.contributions.iter().map(|c| c.1).sum();
        assert!((m.bias + sum - p.logit).abs() < 1e-9);
        assert_eq!(p.risk, round6(sigmoid(p.logit)));
        let factors = m.factors(&p, &f);
        assert!(!factors.is_empty() && factors.len() <= 3);
        assert!(
            factors
                .windows(2)
                .all(|w| w[0].contribution >= w[1].contribution)
        );
        assert!(factors.iter().all(|f| f.contribution > 0.0));
        assert_eq!(factors[0].feature, "kind.breaking");
        assert_eq!(factors[0].text, "2 breaking rows");
    }

    #[test]
    fn stale_models_are_unusable() {
        let m = model_of(&synthetic(90));
        assert_eq!(m.usable(), Ok(()));
        let old = Model {
            feature_set: 0,
            ..m.clone()
        };
        assert!(old.usable().unwrap_err().contains("feature set 0"));
        let mut swapped = m.clone();
        swapped.features.swap(0, 1);
        assert!(swapped.usable().is_err());
        let short = Model {
            weights: vec![0.0],
            ..m
        };
        assert!(short.usable().is_err());
    }

    #[test]
    fn auc_averages_ties() {
        assert_eq!(auc(&[(0.1, false), (0.9, true)]), Some(1.0));
        assert_eq!(auc(&[(0.5, false), (0.5, true)]), Some(0.5));
        assert_eq!(auc(&[(0.5, true)]), None);
    }

    #[test]
    fn plain_words() {
        let ln = |n: f64| ln1(n);
        assert_eq!(describe("kind.breaking", ln(2.0), &[]), "2 breaking rows");
        assert_eq!(describe("blastRadius", ln(38.0), &[]), "blast radius 38");
        let areas = ["billing".to_string(), "orders".to_string()];
        assert_eq!(
            describe("areaProblems", ln(3.0), &areas),
            "3 earlier problems in billing and orders"
        );
        assert_eq!(describe("untested", 0.5, &[]), "50% of rows untested");
        for f in FEATURES {
            assert!(!describe(f, 1.0, &[]).is_empty());
        }
    }
}
