//! Core types of Onus: the codebase map, semantic changes and reports, the
//! `onus.yaml` declared layer, stable ids, ranking and the language adapter
//! interface.
//!
//! Every type serializes to the JSON shapes described in `PLAN.md` section 4
//! and has a generated JSON Schema under `schemas/`.

pub mod change;
pub mod config;
pub mod hash;
pub mod ids;
pub mod map;
pub mod paths;
pub mod plugins;
pub mod protocol;
pub mod provider;
pub mod rank;
pub mod schema;
pub mod secrets;

pub use change::*;
pub use config::*;
pub use map::*;
pub use provider::{
    DiscoveryProvider, FactProvider, LanguageAdapter, PartialMap, ProviderError, Workspace,
    WorkspaceFile, WorkspacePackage,
};

/// Version of Onus, recorded in every map.
pub const ONUS_VERSION: &str = env!("CARGO_PKG_VERSION");
