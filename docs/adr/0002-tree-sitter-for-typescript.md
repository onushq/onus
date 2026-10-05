# 0002. tree-sitter for analyzing TypeScript

Status: Accepted (2026-10-05)

## Context

Phase 1 needs, for TypeScript and JavaScript: components, exported contracts and their shapes, imports, calls resolved to definitions, and a few framework patterns (events, data access, outside services). The engine is Rust (ADR 0001).

## Decision

Parse with tree-sitter (TypeScript and TSX grammars). Onus does its own module resolution (relative paths, index files, tsconfig paths, workspace packages) and name resolution (per-file scope tables plus imports). Contract shapes are read from the syntax: interface members and their optionality, parameter lists, return annotations.

## Alternatives considered

- **oxc**: richer TypeScript semantics in Rust, but TypeScript-only and a faster-moving API.
- **The TypeScript compiler**: precise types, but needs a Node runtime everywhere and an install of the analyzed repository's types.

## Consequences

- One parsing approach for every language we add next, behind the same adapter trait.
- No type checker: where a type is inferred or computed rather than written, the shape is marked `unverified` and the change is treated as higher risk. We will measure how often this happens in the Phase 1 field trial.
