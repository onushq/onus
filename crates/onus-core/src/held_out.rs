//! Held-out suites (`schemas/onus-held-out.schema.json`): checks the judge
//! runs that the author never sees. A suite is a directory outside the
//! repository holding `held-out.yaml` and the files its checks need; the
//! judge copies it into the test container at `/held-out` and picks the
//! checks from the map (`onus help lanes`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `held-out.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HeldOutSuite {
    /// Format version; must be 1.
    pub version: u32,
    /// Default container image for checks. Default `node:22`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Default setup command (with network, without the suite present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// At most this many checks per judgment, most relevant first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    pub checks: Vec<HeldOutCheck>,
}

/// One check of a suite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HeldOutCheck {
    /// A unique slug, shown to people only.
    pub id: String,
    /// Run with `sh -c` in /work, no network; the suite is at /held-out.
    pub command: String,
    /// The container image. Default the suite's `image`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// A setup command run first, with network and without the suite.
    /// Default the suite's `setup`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// Components or map ids the check exercises; empty runs for every change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
}
