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
