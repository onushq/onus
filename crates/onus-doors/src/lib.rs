//! Onus doors (Phase 3, ADR 0008): scoped task tokens and the doors that
//! enforce them, evidence-based escalation and the audit log.

pub mod audit;
pub mod escalation;
pub mod filter;
pub mod gateway;
pub mod plan;
pub mod runner;
pub mod scope;
pub mod token;
