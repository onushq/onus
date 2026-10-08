//! The declared layer: `onus.yaml`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::map::ComponentKind;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OnusConfig {
    /// Format version; must be 1.
    pub version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, ComponentConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub contracts: BTreeMap<String, ContractConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, LabelConfig>,
    #[serde(default)]
    pub extractors: ExtractorsConfig,
    #[serde(default)]
    pub tests: TestsConfig,
    /// Globs of files that are test data, not code: fixtures, sample
    /// projects, recorded responses. They are left out of the map, and
    /// secret-shaped values added in them are noted without counting as
    /// committed secrets.
    #[serde(default, rename = "testData", skip_serializing_if = "Vec::is_empty")]
    pub test_data: Vec<String>,
    /// Risk lanes: what happens to a change after it is reported
    /// (`onus help lanes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<LanesConfig>,
    /// Environments that run the change and record evidence
    /// (`onus help environments`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentConfig>,
}

/// A disposable environment: a container built from the repository at a
/// commit. Read from the operator's working tree, never from the commit
/// under test, so a change cannot alter its own environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EnvironmentConfig {
    /// The container image. Default: the devcontainer's image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Installs dependencies, with network, once per image, setup command
    /// and lockfile contents; the result is kept as a warm image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// Loads synthetic data into a new environment, after setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    /// Globs of test result files (JUnit XML) a run writes, kept as evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// Globs of OpenTelemetry trace files (OTLP JSON) a run writes, kept as
    /// evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traces: Vec<String>,
    /// Files whose contents decide when the warm image is rebuilt. Default:
    /// the usual lockfiles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lockfiles: Vec<String>,
    /// The egress proxy image, started when a task's token names hosts.
    /// Default `ubuntu/squid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_image: Option<String>,
}

/// What happens to a change, from least to most scrutiny.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Lane {
    /// Merged once required checks pass.
    AutoMerge,
    /// Verified by the judge (`onus judge`), then merged or escalated.
    Judge,
    /// A person reviews it.
    Human,
    /// It may not merge as it is.
    Blocked,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::AutoMerge => "auto-merge",
            Lane::Judge => "judge",
            Lane::Human => "human",
            Lane::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LanesConfig {
    /// The lane of a change no rule speaks about. Default `judge`.
    #[serde(default = "default_lane")]
    pub default: Lane,
    /// Rules, in any order: a rule with `match: every` can set a lower
    /// starting lane when every row of the change matches it; every other
    /// rule and the hard floors only move a change up.
    #[serde(default)]
    pub rules: Vec<LaneRule>,
    /// The share of `auto-merge` changes sent to a person anyway, chosen by
    /// a hash of the head commit (0 to 1). Default 0.05.
    #[serde(default = "default_audit_rate")]
    pub audit_rate: f64,
    /// Changes an agent setup needs on record before it may use the
    /// `auto-merge` lane. Default 10.
    #[serde(default = "default_min_record")]
    pub min_record: u32,
    /// Checks the author never saw, run by the judge in a container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_out: Option<CheckCommand>,
    /// An optional reviewer command for taste, run last by the judge. It can
    /// raise concerns that send the change to a person, never approve it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taste: Option<TasteCommand>,
}

fn default_lane() -> Lane {
    Lane::Judge
}

fn default_audit_rate() -> f64 {
    0.05
}

fn default_min_record() -> u32 {
    10
}

impl Default for LanesConfig {
    fn default() -> Self {
        LanesConfig {
            default: default_lane(),
            rules: vec![],
            audit_rate: default_audit_rate(),
            min_record: default_min_record(),
            held_out: None,
            taste: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LaneRule {
    pub lane: Lane,
    /// `any` (default): the rule applies when any row matches. `every`: when
    /// every row matches (and the change has rows).
    #[serde(default, rename = "match")]
    pub match_: RuleMatch,
    /// Row kinds (`internal`, `additive`, `config`, …); empty matches any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// Row subkinds (`migration-changed`, …); empty matches any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subkinds: Vec<String>,
    /// Components; empty matches any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Sensitivity labels; empty matches any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RuleMatch {
    #[default]
    Any,
    Every,
}

/// A command run in a container against the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckCommand {
    /// The command, run with `sh -c` and no network.
    pub command: String,
    /// The container image. Default `node:22`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// A setup command run first, with network (such as `npm ci`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
}

/// A reviewer program run on the operator's machine: it gets the
/// submission (without the author's reasoning) as JSON on stdin and prints
/// one concern per line starting with `concern:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TasteCommand {
    /// The program and its arguments.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentConfig {
    /// Glob (or list of globs) of the component's files, e.g. `services/orders/**`.
    pub path: OneOrMany,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ComponentKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Source files that define the public surface. Inferred from
    /// `package.json` when absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entrypoints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContractConfig {
    /// Map id of the symbol, e.g. `user-preferences:src/types.ts#UserPreferences`.
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleConfig {
    pub deny: RuleEndpointsConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleEndpointsConfig {
    pub from: String,
    pub to: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Sensitivity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelConfig {
    pub sensitivity: Sensitivity,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractorsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<EventPatterns>,
    /// Extra external services, keyed by npm package name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub externals: BTreeMap<String, ExternalSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prisma: Option<PrismaConfig>,
    /// Framework packs: YAML files of tree-sitter queries, relative to
    /// onus.yaml (`onus help plugins`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventPatterns {
    /// Call patterns such as `bus.publish($EVENT, ...)` or `@OnEvent($EVENT)`.
    #[serde(default)]
    pub publish: Vec<String>,
    #[serde(default)]
    pub subscribe: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalSpec {
    pub vendor: String,
    pub category: String,
    #[serde(default)]
    pub egress: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrismaConfig {
    /// Identifiers that hold a Prisma client. Default: `prisma`.
    #[serde(default)]
    pub clients: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestsConfig {
    /// Globs of test files. Defaults cover `*.test.*`, `*.spec.*` and
    /// `__tests__/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub globs: Vec<String>,
}

/// The extractor settings handed to language adapters, with defaults and the
/// built-in registry already merged in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedExtractors {
    pub publish_patterns: Vec<String>,
    pub subscribe_patterns: Vec<String>,
    /// npm package name → service.
    pub externals: BTreeMap<String, ResolvedExternal>,
    pub prisma_clients: Vec<String>,
    /// Framework packs (YAML texts) from onus.yaml and the plugins file,
    /// on top of the built-in ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedExternal {
    pub slug: String,
    pub spec: ExternalSpec,
    pub declared: bool,
}

/// Default event patterns used when `onus.yaml` declares none.
pub const DEFAULT_PUBLISH_PATTERNS: &[&str] = &[
    "bus.publish($EVENT, ...)",
    "eventBus.publish($EVENT, ...)",
    "events.publish($EVENT, ...)",
];
pub const DEFAULT_SUBSCRIBE_PATTERNS: &[&str] = &[
    "bus.subscribe($EVENT, ...)",
    "eventBus.subscribe($EVENT, ...)",
    "events.subscribe($EVENT, ...)",
    "@OnEvent($EVENT)",
];
