# Changelog

All notable changes to Onus are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

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
  - Pluggable map building (ADR 0006): discovery, language and fact providers; external plugins speaking a JSON protocol (`--plugins`); SCIP index import (`--scip`); an LSP bridge for any language server; trusted mode with a sandbox for tools that run repository code; the confidence `compiler`; a reference plugin.
  - `onus help [command | topic]` and a user guide (`docs/guide.md`) that ships in the binary: getting started, commands, reports, kinds of change, `onus.yaml`, the intent check, CI, how it works and troubleshooting. Every command's help has examples, and the commands that can fail on findings list their exit codes.
