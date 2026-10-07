# Changelog

All notable changes to Onus are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

## [0.3.0] - 2026-10-07

- **Sharper reports and `onus_check`:**
  - Untyped values are compared key by key (`Schema.struct({...})`, `z.object({...})`, object literals), and union types member by member.
  - Changes Onus cannot compare are one low-confidence row instead of a "may have changed" row each.
  - Production code that uses an API of an already-used package for the first time is reported with the pinned version.
  - Breaking contract rows name the implementations and test doubles not updated.
  - Changes in components of more than 300 files name their module folders.
- **`onus_check` for agents:** it returns a checklist of what to verify before finishing, and states what it does not verify, so a clean result is evidence, not approval.
- **The map for coding agents (experimental):** `onus mcp` serves the map of a worktree as MCP tools (`onus_status`, `onus_find`, `onus_symbol`, `onus_dependents`, `onus_dependencies`, `onus_tests_for`, `onus_owners`, `onus_component`, `onus_file`, `onus_check`), and `onus query` asks the same questions from a shell. Every agent and worktree of a repository shares one map server, which follows file changes and re-parses only changed files, sharing per-file facts across worktrees (ADR 0007). An [evaluation](docs/evaluation/2026-10-07-map-for-agents.md) found no measurable gain from the navigation tools for agents implementing features, so they are marked experimental and Phase 2 focuses on `onus_check`.
- `onus find` ranks symbols by how many query words match (name, then fields and parameters, then path), finds the types that declare a field, and always lists matching files, instead of returning nothing unless every word was in one name.
- Mapping a large monorepo is about 40% faster (2.6 s to 1.5 s on 8,600 files) with byte-identical maps: public methods are found without a quadratic scan, components are matched with one glob set, packs compile once per process, the file walk is parallel and module resolution is cached.
- The Homebrew tap is updated by a reusable `Homebrew tap` workflow that the release calls. Run it by hand to re-sync the tap, or as a dry run to check that `HOMEBREW_TAP_TOKEN` can still push. Release notes include the `brew install` line.

## [0.2.0] - 2026-10-07

- `testData` in onus.yaml: globs of fixture projects and other sample code. They are left out of the map, their changes are one row, and fake keys added in them are noted without counting as committed secrets, so `fail-on: secrets` does not trip on them.
- `onus report --cache-dir`: keep the base ref's map and reuse it on the next report against the same base commit. The action caches it in the Actions cache (input `cache`).
- The action ends its comment with a 👍/👎 and `/onus caught` feedback line and keeps the report's metrics in it; it also acknowledges `/onus caught` replies on `issue_comment`. `scripts/onus-metrics.sh` collects the Phase 1 measures from a repository's pull requests.
- Homebrew: `brew install onushq/tap/onus`. The release workflow updates the tap.
- A Markdown intent file (such as a pull request body) is only read for its `onus-intent` block.
- Help shows the command as `onus` on every platform.

## [0.1.0] - 2026-10-07

- Release binaries for Linux (static, x86_64 and ARM64), macOS (Apple silicon and Intel) and Windows, with SHA-256 checksums and build attestations; a shell installer (`curl -fsSL https://onushq.com/install.sh | sh`).
- The GitHub Action (`onushq/onus/action`): downloads the release, writes the report to the job summary, keeps one pull request comment up to date, checks the pull request body's intent and fails on `fail-on` findings. Onus reports on its own pull requests with it.
- Project plan, open source setup and decision records.
- First version of Phase 1 (milestones M0 to M5):
  - Cargo workspace with `onus-core`, `onus-lang-ts`, `onus-map`, `onus-diff`, `onus-report` and `onus-cli`; CI on Linux, macOS and Windows; `cargo deny` license checks.
  - `onus-core`: the codebase map, semantic change and report types, the `onus.yaml` types, stable ids, ranking, secret patterns and the `LanguageAdapter` trait. JSON Schemas are generated into `schemas/`.
  - TypeScript and JavaScript adapter on tree-sitter: imports and exports with re-exports, functions, interfaces, type aliases, classes, enums and consts with normalized shapes, body fingerprints, scope-aware call resolution, module resolution (relative paths, index files, `tsconfig` paths, workspace packages), events from configurable patterns, Prisma reads and writes, `process.env` reads, external SDKs and literal HTTP hosts, comparison and guard facts, and test cases with assertions and skip/only/todo markers.
  - Map building: components from `onus.yaml`, workspaces or folders; owners from `CODEOWNERS`; public surface from package entrypoints; a built-in registry of external services; package dependencies; boundary rules; `onus init`.
  - Semantic diff: renames and moves, contract changes, new relationships (events, data, external services, components), dependencies, config and rules-of-the-game files, weakened tests, notable edits such as flipped comparisons, committed secrets, new boundary-rule violations, the intent check, ranking and one internal row per component.
  - Markdown and JSON reports; `onus map`, `onus diff`, `onus report`, `onus init` and `onus schema`, with `--fail-on rule-violation|secrets`.
  - The `fixtures/shop` monorepo with golden scenarios S1 to S9, snapshotted and checked.
  - Field-tested on a large Nx/pnpm monorepo: Nx projects as components, one row for symbols moved between components (also when edited on the way), grouped config and dependency rows, pnpm `catalog:` versions, assets and SvelteKit `$lib` imports, tests kept out of relationships, and a fix for exponential time on deep call chains.
  - Svelte support as a plugin (`onus-plugin-svelte`); the LSP bridge waits for servers to load the project (tried with rust-analyzer and pyright); SCIP import tried with rust-analyzer and scip-typescript on real repositories.
  - Framework packs: YAML files of tree-sitter queries that add routes, events, data access and other relationships without code, from onus.yaml or the plugins file; Onus's own Prisma, `process.env`, HTTP-client and event extractors are now packs. Cargo and Python projects are discovered as components; a `container` sandbox preset; real sandbox tests in CI.
  - Pluggable map building (ADR 0006): discovery, language and fact providers; external plugins speaking a JSON protocol (`--plugins`); SCIP index import (`--scip`); an LSP bridge for any language server; trusted mode with a sandbox for tools that run repository code; the confidence `compiler`; a reference plugin.
  - `onus help [command | topic]` and a user guide (`docs/guide.md`) that ships in the binary: getting started, commands, reports, kinds of change, `onus.yaml`, the intent check, CI, how it works and troubleshooting. Every command's help has examples, and the commands that can fail on findings list their exit codes.
