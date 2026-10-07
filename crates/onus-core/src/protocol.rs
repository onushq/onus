//! The plugin protocol (ADR 0006): Onus starts a plugin's command, writes
//! one [`PluginRequest`] as JSON to its stdin and reads one
//! [`PluginResponse`] as JSON from its stdout. Everything a plugin returns
//! uses the map's own types. The schema is `schemas/plugin-protocol.schema.json`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::map::Component;
use crate::provider::{PartialMap, Workspace};

/// Version of the plugin protocol. Plugins reject versions they don't know.
pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum PluginKind {
    /// Finds components.
    Discovery,
    /// Turns the files it handles into symbols, references and tests.
    Language,
    /// Adds facts to the map built so far.
    Facts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginRequest {
    pub protocol: u32,
    pub kind: PluginKind,
    /// For `language`, `files` lists only the files this plugin handles;
    /// every file under `root` can still be read.
    pub workspace: Workspace,
    /// For `facts`: the map built so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<PartialMap>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginResponse {
    pub protocol: u32,
    /// The plugin's own version, recorded in the map's `builtWith`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For `discovery`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    /// For `language` and `facts`.
    #[serde(default)]
    pub map: PartialMap,
}

/// The request and response together, for schema generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginProtocol {
    pub request: PluginRequest,
    pub response: PluginResponse,
}
