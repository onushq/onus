//! Builds a [`CodebaseMap`](onus_core::CodebaseMap) from a tree of files:
//! discovers components (from `onus.yaml`, workspaces or folders), assigns
//! owners, labels and tests, runs the language adapters and adds the
//! declared layer (rules, contracts, external services).

pub mod build;
pub mod config;
pub mod discover;
pub mod init;
pub mod merge;
pub mod plugin;
pub mod registry;
pub mod walk;

pub use build::{BuildOptions, build_map};
pub use config::LoadedConfig;
pub use onus_lang_ts::{CacheStats, FactsCache};

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("invalid onus.yaml: {0}")]
    Config(String),
    #[error("cannot read {0}")]
    Io(String),
}
