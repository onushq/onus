# 0007. An in-memory map server for agents, without a graph database

Status: Proposed (2026-10-07)

## Context

Phase 2 serves the map to agents through MCP and a CLI. The expected load is many agents working on one repository at the same time, each in its own git worktree, asking questions while they edit. Answers must reflect the worktree as it is, come back fast enough to sit inside an agent's loop, and stay deterministic and bounded like the rest of Onus.

Graph databases were considered for storing and querying the map (Grafeo, HelixDB, OverGraph, HydraDB, SurrealDB, IndraDB, FalkorDB). On a large TypeScript monorepo (8,600 source files) the map has 31,000 symbols and 115,000 edges. That is small for a graph engine, and the planned questions (dependents, dependencies, tests, owners, impact) walk one to a few steps from a node. The map is also derived per commit from git, which is the source of truth, rather than a long-lived store that is edited.

## Decision

1. **No graph database.** The map stays an immutable snapshot of a tree (D4). An index (`onus-index`) holds it in memory with adjacency lists in both directions and lookups by symbol, file, module and component. It answers typed queries, not a query language: results are sorted, limited and carry their evidence, and each query is the same code for the CLI (`onus query`) and MCP (`onus mcp`).
2. **One map server per repository.** `onus mcp` and `onus query` talk to a server over a Unix socket in a private per-user folder, named by the repository's git folder, and start it when it is not running. It keeps one session per worktree and exits when idle. Without Unix sockets (Windows) the same server runs in the calling process.
3. **Incremental by file content.** Per-file facts are cached by a hash of the file's path, bytes and the extraction settings, in memory and in `.git/onus/facts`, which every worktree of the repository shares. A rebuild re-parses only files whose content is new, then links the whole workspace again, so the map is identical to a fresh build. A file watcher marks a worktree's map stale; the next question rebuilds it once for every waiting caller.
4. **The map is built the same way everywhere.** The server uses the same `build_map` as `onus map` and `onus report`; caching is an option of the build, not a second implementation.

## Measurements

On the monorepo above (Apple M-series laptop), after the optimizations this work required (a quadratic pass in public-symbol detection, per-component glob matching, packs compiled twice, a serial file walk, uncached module resolution), with byte-identical maps before and after:

| | Before | After |
|---|---|---|
| `onus map`, fresh process | 2.6 s | 1.5 s |
| MCP tool call, warm map | | under 5 ms |
| First question in a second worktree | | 0.9 s |
| Question after an edit (rebuild) | | 0.7 s |
| Eight concurrent agents after an edit | | 0.7 s, one rebuild |

## Consequences

- One process holds the maps; memory grows with the number of active worktrees (about 0.5 GB for the monorepo above with one worktree and a base commit).
- A rebuild after an edit still links the whole workspace (about 0.3 s of the 0.7 s here). Incremental linking (re-linking only changed files and the files whose imports reach them) is the next step if rebuilds become the bottleneck.
- If a hosted, multi-repository service ever outgrows memory, an embedded engine can replace the index behind the same queries; only embeddable engines under permissive licenses fit Onus's single binary.
- The server is read-only and runs nothing from the repository. A socket readable only by the user is the access control until Phase 3 adds scoped tokens.
