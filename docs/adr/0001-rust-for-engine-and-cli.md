# 0001. Rust for the engine and the CLI

Status: Accepted (2026-10-05)

## Context

Onus runs in CI on every pull request, on developer machines, and inside agent sandboxes. It has to map large monorepos quickly and must never execute the code it analyzes. Later phases add an MCP server, a git gateway and a token service that check every call.

## Decision

Write the engine and the `onus` CLI in Rust, as one Cargo workspace that ships a single static binary per platform. The MCP server (Phase 2) uses the official Rust MCP SDK and ships in the same binary.

## Consequences

- No runtime to install anywhere; the GitHub Action just downloads the binary.
- Parallel parsing makes per-pull-request mapping fast enough to run on every change.
- Strong types for the schemas everything else depends on; JSON Schema is generated from them.
- We give up the TypeScript compiler's type checker for TS analysis (see ADR 0002).
- Contributors need Rust; we keep the code approachable with clear crate boundaries and snapshot tests.
