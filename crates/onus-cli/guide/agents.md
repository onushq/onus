# The map for coding agents (MCP)

`onus mcp` gives a coding agent the codebase map as MCP tools: where things are, what depends on what, which tests cover what, who owns it, and what a change means before it is committed. Every answer is computed from the code, never generated, and carries the file and line it comes from.

The tools are read-only.

**What helps agents today is `onus_check`.** We tested whether the map makes coding agents faster or more correct when they implement a feature ([evaluation](https://github.com/onushq/onus/blob/main/docs/evaluation/2026-10-07-map-for-agents.md)). It did not, measurably: agents found code with text search just as well and rarely called the navigation tools. They did call `onus_check` before finishing, and Phase 2 now focuses on making it precise. The navigation tools below are experimental.

## Connect an agent

Claude Code:

    claude mcp add onus -- onus mcp

Any MCP client that starts servers over stdio: run `onus mcp` in the repository (or pass `--repo <worktree>`). The map is the map of the worktree the server is started in, as it is on disk now, saved but not committed changes included.

Clients that connect over HTTP (streamable HTTP): run `onus mcp --http 127.0.0.1:8765` and point the client at `http://127.0.0.1:8765/`. Only requests naming a loopback host are answered, which keeps other machines and DNS rebinding out; `--allow-host <name>` adds a host for a network you trust. The tools are read-only but describe your code, and there is no authentication. Every tool answers the same over stdio and HTTP, and to every client.

## Tools

The check:

- `onus_check`: the changes in meaning between a commit (default `HEAD`) and the worktree now, as the same rows as a pull request report, preceded by a short checklist of what to verify before finishing: third-party APIs the repository has never used (with the pinned version), implementations and test doubles of a changed interface that were not updated, committed secrets and broken boundary rules. Run it before finishing a task.
- `onus_impact`: before a change, what breaks if a symbol, file or component is removed, renamed, changes its signature, gains a required member, or only changes behavior. One site per file (file, line, how it is used; for a new required member only the implementations, found in the source text), the tests that exercise it, and what static analysis cannot see.
- `onus_invariants`: the invariants onus.yaml declares for a symbol, file or component, or all of them: rules a change must keep.

Navigation (experimental):

- `onus_status`: what the map holds (components, files, symbols, edges, tests). Start here.
- `onus_find`: symbols by name words, ranked by how many words match in the name, then in fields and parameters, then in the file path (`format phone` finds `formatPhoneNumber`; a field name finds the types that declare it). Every answer also lists matching files.
- `onus_symbol`: one symbol: kind, location, contract shape, declared invariants, how many places use it.
- `onus_dependents`: what depends on a symbol, file or component, up to `depth` steps (imports, calls, type references). Use it before changing something.
- `onus_dependencies`: what a symbol, file or component depends on: code, external services, events, tables, config keys.
- `onus_tests_for`: the test files that exercise a symbol, file or component.
- `onus_owners`: owners and sensitivity labels.
- `onus_component`: a component's public surface, the components it uses and that use it, its external services and events.
- `onus_file`: a file's symbols, imports and importers.

Targets can be a symbol id (`billing:src/payments.ts#chargeCard`), a bare symbol name, a file path, a module id or a component id. Answers are JSON, sorted, and limited (default 50 items, `limit` up to 500); `truncated` says when there is more. Each answer also says which version of the map it came from and whether the map was rebuilt for it.

The same questions work from a shell, with the same answers:

    onus query status
    onus query find format phone
    onus query dependents UserPreferences --depth 2
    onus query tests-for services/billing/src/payments.ts
    onus query impact UserPreferences --change add-required-member
    onus query invariants
    onus query check --base main

## Many agents, many worktrees

Agents often work in parallel, each in its own git worktree of one repository. Onus is built for that:

- **One server per repository.** Every `onus mcp` and `onus query` of a repository talks to one map server over a private Unix socket, and starts it if it is not running. Ten agents in a worktree share one map and one rebuild.
- **One map per worktree, kept current.** The server watches each worktree's files. A change marks the map stale; the next question rebuilds it once, however many agents are asking, and questions between changes are answered from memory in about a millisecond.
- **Facts shared across worktrees.** Parsing a file is most of the work, and worktrees share almost all of their files. The server keeps each file's facts by content, in memory and in the repository's git folder (`.git/onus/facts`), so a second worktree, a rebuild after an edit, or a restarted server parses only files whose content is new.
- **Checks reuse base commits.** `onus_check` keeps the extracted base commit and its map, so agents checking against the same commit pay for it once.

Measured on Twenty, an open-source TypeScript monorepo with 31,000 source files, 72,000 symbols and 272,000 edges (Apple M-series laptop):

| | Time |
|---|---|
| A question to a warm map (MCP tool call) | under 1 ms; `onus_find` about 30 ms |
| The same from `onus query` (a new process each time) | 20–50 ms |
| First question in a repository (full build) | 4.8 s |
| First question in a second worktree | 1.9 s |
| First question after an agent edits a file | 2.0 s |
| Eight agents asking at once after an edit | 1.7 s, one rebuild |
| `onus_check` against a commit already checked | 2.4 s |

The server exits after 30 minutes without questions. `onus query stats` shows its worktrees and cache use; `--no-server` builds the map in the calling process instead (the only mode on Windows).

## What it reads and runs

The same as every other command: the files of the worktree and, for `onus_check`, the base commit through `git archive`. Nothing from the repository is executed, the server only listens on a socket readable by your user, and no plugins are loaded.
