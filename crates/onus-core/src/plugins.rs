//! The plugins file (ADR 0006): which plugins to run and how to sandbox
//! the ones that may run repository code. It comes from whoever runs Onus
//! (`--plugins` or `ONUS_PLUGINS`), never from the analyzed repository.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The plugins file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginsFile {
    #[serde(default)]
    pub plugins: Vec<PluginSpec>,
    #[serde(default)]
    pub sandbox: SandboxSpec,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SpecKind {
    /// Speaks the plugin protocol and finds components.
    Discovery,
    /// Speaks the plugin protocol and analyzes the files it handles.
    Language,
    /// Speaks the plugin protocol and adds facts to the map.
    Facts,
    /// A SCIP indexer: writes an index to `{out}/index.scip`, which Onus
    /// imports.
    Scip,
    /// A language server: Onus talks LSP to it over stdio.
    Lsp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginSpec {
    /// Recorded in the map's `builtWith.providers`.
    pub name: String,
    pub kind: SpecKind,
    /// The command and its arguments. `{root}` is replaced by the tree's
    /// root and `{out}` by a fresh, writable temporary directory.
    pub command: Vec<String>,
    /// For `language` and `lsp`: globs of the files to hand over.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// For `lsp`: the LSP language id of those files (`python`, `go`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_id: Option<String>,
    /// Whether the tool may run code from the analyzed repository (build
    /// scripts, macros, project plugins). Default: true for `scip` and `lsp`,
    /// false otherwise. Such plugins only run in trusted mode, in a sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs_repo_code: Option<bool>,
    /// Default: 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl PluginSpec {
    pub fn runs_repo_code(&self) -> bool {
        self.runs_repo_code
            .unwrap_or(matches!(self.kind, SpecKind::Scip | SpecKind::Lsp))
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds.unwrap_or(300))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxPreset {
    /// bubblewrap on Linux, sandbox-exec on macOS, nothing elsewhere.
    #[default]
    Auto,
    Bwrap,
    SandboxExec,
    /// A container with no network, the tree mounted read-only and `{out}`
    /// writable; needs `image` (with the tools in it) and Docker or Podman.
    Container,
    /// No sandbox: plugins that run repository code then need
    /// `--allow-unsandboxed`.
    None,
}

/// How to sandbox plugins that run repository code: a preset, or a custom
/// command prefix (for example a container runtime) with `{root}` and
/// `{out}` placeholders.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxSpec {
    #[serde(default)]
    pub preset: SandboxPreset,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// For `container`: the image to run plugins in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// For `container`: `docker` or `podman` (default: whichever is on the
    /// PATH, Docker first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}
