//! Environments and the evidence store (ADR 0010).
//!
//! An environment is a container built from the repository at a commit; a
//! command run in it leaves a manifest and its artifacts in a content
//! addressed store, so evidence is something an environment records rather
//! than something an agent writes down.

pub mod env;
pub mod spec;
pub mod store;
