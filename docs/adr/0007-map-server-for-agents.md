# 0007. An in-memory map server for agents, without a graph database

Status: Proposed (2026-10-07); amended the same day after an evaluation (see the end)

## Context

Phase 2 serves the map to agents through MCP and a CLI. The expected load is many agents working on one repository at the same time, each in its own git worktree, asking questions while they edit. Answers must reflect the worktree as it is, come back fast enough to sit inside an agent's loop, and stay deterministic and bounded like the rest of Onus.

Graph databases were considered for storing and querying the map (Grafeo, HelixDB, OverGraph, HydraDB, SurrealDB, IndraDB, FalkorDB). On Twenty, an open-source TypeScript monorepo with 31,000 source files, the map has 72,000 symbols and 272,000 edges. That is small for a graph engine, and the planned questions (dependents, dependencies, tests, owners, impact) walk one to a few steps from a node. The map is also derived per commit from git, which is the source of truth, rather than a long-lived store that is edited.

## Decision

1. **No graph database.** The map stays an immutable snapshot of a tree (D4). An index (`onus-index`) holds it in memory with adjacency lists in both directions and lookups by symbol, file, module and component. It answers typed queries, not a query language: results are sorted, limited and carry their evidence, and each query is the same code for the CLI (`onus query`) and MCP (`onus mcp`).
2. **One map server per repository.** `onus mcp` and `onus query` talk to a server over a Unix socket in a private per-user folder, named by the repository's git folder, and start it when it is not running. It keeps one session per worktree and exits when idle. Without Unix sockets (Windows) the same server runs in the calling process.
3. **Incremental by file content.** Per-file facts are cached by a hash of the file's path, bytes and the extraction settings, in memory and in `.git/onus/facts`, which every worktree of the repository shares. A rebuild re-parses only files whose content is new, then links the whole workspace again, so the map is identical to a fresh build. A file watcher marks a worktree's map stale; the next question rebuilds it once for every waiting caller.
4. **The map is built the same way everywhere.** The server uses the same `build_map` as `onus map` and `onus report`; caching is an option of the build, not a second implementation.

## Measurements

On Twenty (Apple M-series laptop), after the optimizations this work required, with byte-identical maps before and after. The optimizations:

- removing a quadratic pass in public-symbol detection;
- matching components with one glob set instead of one per component;
- compiling packs once instead of twice;
- walking files in parallel;
- caching module resolution.

| | Before (0.2.0) | After |
|---|---|---|
| `onus map`, fresh process | 4.6 s | 3.5 s |
| A question to a warm map, from a new `onus query` process | | 20–50 ms |
| First question in a second worktree | | 1.9 s |
| Question after an edit (rebuild) | | 2.0 s |
| Eight concurrent agents after an edit | | 1.7 s, one rebuild |
| `onus_check` again on the same base commit | | 2.4 s |

## Consequences

- One process holds the maps; memory grows with the number of active worktrees (about 1.3 GB for Twenty with one worktree and a base commit; the facts cache on disk is about 210 MB).
- A rebuild after an edit still links the whole workspace, which is most of its time. Incremental linking (re-linking only changed files and the files whose imports reach them) is the next step if rebuilds become the bottleneck.
- If a hosted, multi-repository service ever outgrows memory, an embedded engine can replace the index behind the same queries; only embeddable engines under permissive licenses fit Onus's single binary.
- The server is read-only and runs nothing from the repository. A socket readable only by the user is the access control until Phase 3 adds scoped tokens.

## Amendment: evaluation with coding agents (2026-10-07)

We tested whether the map helps coding agents, in [an A/B evaluation](../evaluation/2026-10-07-map-for-agents.md): Claude Code (Sonnet) implemented a real Twenty feature with and without `onus mcp`. A blind reviewer graded the results against the shipped commit.

- **No measurable gain in speed, cost or quality.** On Twenty, the baseline averaged 235 s, $1.40 and a score of 7.8/10; Onus averaged 231 s, $1.49 and 7.5/10. The spread within each condition was larger than the gap between them.
- **Agents did not use the navigation tools**, even when the prompt asked them to. Text search finds code just as well in well-named repositories.
- **`onus_check` was the tool agents used unprompted**, but its output was too noisy or too coarse to act on.

This changes what Phase 2 is for, not how the server works:

1. The server and the facts cache stay, for `onus_check` and later verification tools. The faster map building stays too, and speeds up `onus report` for everyone.
2. The navigation tools (`onus_find`, `onus_symbol`, `onus_dependents`, `onus_dependencies`, `onus_tests_for`, `onus_owners`, `onus_component`, `onus_file`) are **experimental**. They are kept for people and for later tests, but Phase 2 is not built around them.
3. Phase 2 focuses on **verification**, meaning facts an agent cannot get from text search, delivered through `onus_check` and the Phase 1 report. The next steps:
   - drop unverified "inferred type" rows;
   - flag external APIs the repository has never used, with the pinned version;
   - list every implementer and test double of a changed interface;
   - split large packages into components by module.
4. The gate for Phase 2 becomes a repeat of this evaluation, with at least three runs per condition, on tasks where verification is the hard part (reviewing or extending an existing change). Onus must show a measurable gain in correctness to proceed.

### Second evaluation and the role of MCP (2026-10-07)

After `onus_check` was made precise, a second evaluation (3 runs per condition, [same document](../evaluation/2026-10-07-map-for-agents.md#second-evaluation-a-precise-onus_check)) did not show a gain. The checklist was empty, because the task used no new APIs and broke no interfaces. The Onus runs scored 6.3 against the baseline's 8.0, and stopped sooner, so `onus_check` now states what it does not verify.

Onus does not compete with agents or their harnesses. Users bring any agent, and Onus assesses the risk of each change and asks agents for evidence. The MCP server is therefore an **evidence channel**: `onus_check` returns verified facts and a checklist to answer, and says what it does not verify, so a clean result is never read as approval. The navigation tools remain experimental.

## Amendment (2026-10-08): Phase 2 completed

- `impact(target, change)` and `invariants(target)` are typed queries like the others, served by `onus_impact` and `onus_invariants`. `impact` reads source text only for one question, which files implement a type, using the same detection as the diff (now in `onus-core`); the index is given the worktree root for it.
- The MCP server also speaks streamable HTTP (`onus mcp --http`), with `rmcp`'s server behind a minimal `hyper` loop. It answers only requests naming a loopback host unless more are allowed, since the tools describe the code and have no authentication; Phase 3's tokens are the way to open it wider.
- The gate is a references benchmark against the TypeScript language service rather than hand-written question and answer pairs: the compiler gives ground truth for every symbol, so the sample can be large and rerun on any repository.

