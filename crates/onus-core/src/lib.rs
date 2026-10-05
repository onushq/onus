//! Core types of Onus: the codebase map, semantic changes and reports, the
//! `onus.yaml` declared layer, stable ids, ranking and the language adapter
//! interface.
//!
//! Every type serializes to the JSON shapes described in `PLAN.md` section 4
//! and has a generated JSON Schema under `schemas/`.

pub mod adapter;
pub mod change;
pub mod config;
pub mod hash;
pub mod ids;
pub mod map;
pub mod paths;
pub mod rank;
pub mod schema;
pub mod secrets;

pub use adapter::{LanguageAdapter, PartialMap, Workspace, WorkspaceFile, WorkspacePackage};
pub use change::*;
pub use config::*;
pub use map::*;

/// Version of Onus, recorded in every map.
pub const ONUS_VERSION: &str = env!("CARGO_PKG_VERSION");
