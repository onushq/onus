//! Onus risk lanes (Phase 4, ADR 0009): change submissions, the rules-first
//! classifier, the learned model that may only raise lanes (ADR 0015), the
//! verifying judge with held-out suites and rotating configurations (ADR
//! 0018), outcome records, production signals and rollout plans (ADR 0016).

pub mod classify;
pub mod heldout;
pub mod judge;
pub mod learned;
pub mod outcomes;
pub mod provenance;
pub mod pulls;
pub mod review;
pub mod rollout;
pub mod signals;
pub mod submission;
