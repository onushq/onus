# Changelog

All notable changes to Onus are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

## [0.10.0] - 2026-10-09

Track records that fill themselves in, and agents that work through Onus.

- **Agents hand work in through MCP** ([ADR 0012](docs/adr/0012-agents-act-through-mcp.md), `onus help agents`): `onus mcp` adds `onus_env_create`, `onus_env_run`, `onus_envs`, `onus_env_destroy`, `onus_evidence`, `onus_run_test`, `onus_submit`, `onus_judge`, `onus_escalate`, `onus_escalation`, `onus_lanes` and `onus_outcomes`. Agents cannot record outcomes, grant escalations, mint tokens or choose their scope; `--no-actions` leaves the tools out. Submissions are kept in `.onus/submissions/` and shown in `onus ui`; granted escalation tokens are kept beside their request for the agent to collect.
- **Outcome records kept by CI** ([ADR 0013](docs/adr/0013-outcome-records-from-ci.md), `onus help ci`): with `records: true`, the GitHub Action classifies every pull request with its agent setup's track record, records the outcome when it closes and reverts when the default branch moves, all on an `onus/records` branch of the repository. People record approvals, audits and incidents with `/onus approve <row>`, `/onus audit ok|miss` and `/onus incident <note> involved: …` comments (write access only); an approval re-runs the check. The agent setup is read from an `Onus-Agent:` trailer, the branch name, the bot author or a `Co-authored-by` trailer. New `onus ci pr|closed|comment|push|records` commands do the work and run in any CI system. An incident now counts against its agent setup even after the change is reverted.
- **An outcomes dashboard in `onus ui`**: reads `origin/onus/records` when there is no local file, and shows changes by week and lane, each agent setup's progress toward auto-merge, and the pull requests in flight.
- **A better map in `onus ui`**: pan and zoom, a layout by folder beside the layers, focus on a component with one or two steps of neighbours, filters for imports, calls and types, a side panel for the selected component, and a treemap of the repository by folder and size.

## [0.9.0] - 2026-10-09

A web interface for everything Onus does.

- **`onus ui`**: a web interface for the repository, embedded in the binary and served on 127.0.0.1 behind a per-session token ([ADR 0011](docs/adr/0011-web-interface.md), `onus help web-interface`): a dashboard with the component graph, a command palette (⌘K), and light and dark themes. It covers the component graph and every component, symbol and file (dependents, dependencies, tests, invariants and the impact of a change); reports of uncommitted work and of any two refs; the lane policy, submissions, classification and the judge; environments, runs and the evidence store; outcomes, incidents and reverts; root keys, task plans, minting, inspecting and narrowing tokens; escalations; the audit log; onus.yaml; and the guide.
- JUnit results are counted by their `<testcase>` elements, so reporters that write no suite totals (such as Node's) are counted.
- Lane and judge reasons quote row titles instead of wrapping them in backticks, which broke the code spans inside them.

## [0.8.0] - 2026-10-09

Phase 5 complete: environments, the evidence store, traces, the production loop and contract-level scopes ([ADR 0010](docs/adr/0010-evidence-factory.md), `onus help environments`).

- **Environments:** `onus env create|run|list|destroy` builds a container from a commit and `environment:` in onus.yaml (or the devcontainer). Setup runs once per lockfile and is kept as a warm image; a seed command loads synthetic data. There is no network unless the task's token names hosts, and then only through an egress proxy that allows exactly those hosts. Secrets the token names come from `ONUS_SECRET_<NAME>` and are redacted from everything stored.
- **The evidence store:** each run leaves a manifest and its artifacts (the log, JUnit XML, OTLP traces) under their sha256 in the repository's git directory; `onus evidence list|show` reads it. `onus env run` prints a test run for `onus submit --evidence`, and the judge checks it against its manifest.
- **Traces feed the map:** `--traces <otlp.json>` on `onus map`, `diff` and `report` adds `traced` call edges from OpenTelemetry spans, such as an event bus dispatching to a handler.
- **The production loop:** `onus outcomes ingest-reverts` finds reverts in git history, `onus outcomes incident` records incidents, and `--result rolled-back` records flags turned off and rollbacks. Reverted changes leave an agent setup's record, a recent incident stops it from auto-merging, and `onus outcomes backlog` lists where held-out tests should go.
- **Contract-level scopes:** plans can name `writeComponents` (`notifications.internal`), `readContracts` and `escalateBefore: ["contract:*"]`; `onus token mint --repo` resolves them through the map, and the gateway refuses changes to guarded contract files without a granted escalation.

### Fixed

- `onus run-test` and the judge's re-runs saw an empty tree with container engines that run in a VM (colima, podman machine), which do not share the host's temporary directories. The commit's files are now copied into the container instead of mounted.

## [0.7.0] - 2026-10-08

Phase 4 complete: risk lanes and the verifying judge ([ADR 0009](docs/adr/0009-risk-lanes-and-the-judge.md), `onus help lanes`).

- `onus submit` bundles a change for review: the intent, Onus's report, changed paths, test runs to re-run, the token's scope, escalations, approvals and the agent setup, and nothing of the author's reasoning.
- `onus classify` puts a change in a lane (`auto-merge`, `judge`, `human`, `blocked`). The lane comes from a policy in onus.yaml (`lanes:`) and hard floors no policy can lower: secrets, out-of-scope writes and unapproved weakened tests are blocked; sensitive code, rules of the game and intent mismatches go to a person. New agent setups cannot auto-merge, and a share of auto-merges is audited by a person.
- `onus judge` verifies before it judges: it re-runs the evidence in containers, then checks intent, contracts and tests, then runs held-out checks. An optional taste reviewer can raise concerns but never approve. The verdict is approve, reject (with reasons the agent can act on) or escalate.
- The action's `lanes`, `apply-lane` and `auto-merge` inputs show the lane, label the pull request, fail blocked changes and enable GitHub's auto-merge.
- `onus outcomes record|summary` track outcomes per agent setup and judge configuration, including the audit miss rate and the human-lane share.

## [0.6.0] - 2026-10-08

Phase 3 complete: scoped tokens, enforcing doors and evidence-based escalation ([ADR 0008](docs/adr/0008-scoped-tokens-and-doors.md), `onus help scopes`).

- **Task tokens:** `onus token keygen|mint|attenuate|inspect|check` mint Biscuit tokens from task plans (writes, reads, hosts, secrets, refs, ttl). Holders can narrow a token for a sub-agent but never widen it. `onus scope suggest` proposes reads from the map, leaving out sensitive components.
- **The git gateway:** `onus gateway serve` gives each task a mirror holding only what it may read. It refuses pushes that change anything outside the write scope, with an error that says how to ask, and replays accepted commits onto the real repository with a credential the agent never holds. A red-team test suite is the gate: zero out-of-scope writes.
- **Read scope for the map:** `onus mcp --token` answers only about files the task may read.
- **Escalation:** `onus escalate` records a structured request with graded evidence, the blast radius and sensitive labels. `onus escalation decide` grants low-risk requests automatically when a failing test is reproduced by `onus run-test` (a container with no network); `grant` and `deny` are for people.
- **Audit:** every decision is a hash-chained JSON line; `onus audit <log>` checks the chain.

## [0.5.0] - 2026-10-08

Phase 2 complete:

- `onus_impact` (`onus query impact <target> --change remove|rename|change-signature|add-required-member|change-behavior`): before a change, the files that break or need a check, one site each (for a new required member only the implementations, found in the source text), and the tests that exercise the target.
- `onus_invariants` (`onus query invariants [target]`): the invariants onus.yaml declares.
- `onus mcp --http <addr>`: the MCP tools over streamable HTTP. Only loopback host names are answered unless `--allow-host` adds one; every tool answers the same over stdio and HTTP.
- `scripts/benchmark-references.mjs`: compares the map's dependents with the TypeScript compiler's find-all-references. Recall is 1.000 on the fixture and 0.999 on Twenty ([benchmark](docs/evaluation/2026-10-08-references-benchmark.md)).

## [0.4.1] - 2026-10-08

- From replaying every commit of three SvelteKit and TypeScript monorepos:
  - Private declarations that share a name or a small body in different files are no longer reported as renames or moves.
  - A type that only gains optional parameters or fields is a compatible change (`contract-type-widened`).
  - Drizzle migrations are recognized, with the indexes they create.
  - Awaits merged into `Promise.all`, rewritten away or deduplicated are not "removed".
  - Guards that moved with renamed variables are matched by their shape.
  - A new package added to several components is one row.
  - Route rows name their component, and first-use rows skip `$app` and `$env`.
- `--markdown-out <file>` writes the Markdown report alongside the JSON, so the action runs each report once instead of twice.
- The action's `plugins` input passes a plugins file (such as the Svelte plugin) without reading it from the analyzed repository.

## [0.4.0] - 2026-10-08

M7 tuning, from an audit of reports on 20 large real pull requests: fewer wrong facts, flags on the changes that need a person, less noise, faster reports. Audited again afterwards, rows with wrong facts went from 7% to under 1% and the reviewers' usefulness score from 2.1 to 3.0 out of 5 ([evaluation](docs/evaluation/2026-10-08-m7-replay.md)). A regression corpus (`crates/onus-cli/tests/corpus`) reproduces each kind of mistake with invented code and keeps it fixed.

- **Fewer false alarms:**
  - A contract change is breaking only when a user or implementation outside the change was not updated. Casts (`as X`) no longer count as implementations; published packages and packages with unresolved imports stay breaking.
  - Types are compared with object and union members sorted, so regenerated code that only reorders is no change.
  - An await, guard or throw that moved into another function (a body extracted into a helper) is not reported as removed.
  - Tests that moved, or went with the code they tested, no longer read as weakened; the latter get a quiet `tests-removed-with-code` row.
  - Imports of package subpaths (`pkg/utils`) resolve through `exports`, mapping build output back to source, so users of shared packages are no longer missed.
  - Lockfile rows list the installed versions that change, or say none do (patch hashes and checksums are not versions).
  - First-use rows skip Node's built-in modules and APIs already used through a namespace.
- **Flags by risk class**, each its own row that needs a person: migrations (with what they do to stored data), GraphQL operations (separately when they skip authentication), HTTP routes from file-based routing and controllers, authentication and authorization code, patched dependencies, root build config and infrastructure. Rows that need a person rank first.
- **Less noise:** generated code is one quiet row per component; config rows name the keys or Dockerfile instructions that changed; build-tool and workspace files have their own classes instead of "runtime configuration"; widespread config edits are grouped by the keys they change.
- **Faster reports:** `onus report` compares only the paths git reports as different, writes both trees on all cores, shares parsed facts between the two maps, and with a cached base map extracts only the base's changed files. On a 31,000-file repository a report takes about 22 s instead of 29 s, and about 12 s with the cache.

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
- Mapping a large monorepo is about 25% faster (4.6 s to 3.5 s on Twenty's 31,000 files) with byte-identical maps: public methods are found without a quadratic scan, components are matched with one glob set, packs compile once per process, the file walk is parallel and module resolution is cached.
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
  - Monorepo support: Nx projects as components, one row for symbols moved between components (also when edited on the way), grouped config and dependency rows, pnpm `catalog:` versions, assets and SvelteKit `$lib` imports, tests kept out of relationships, and a fix for exponential time on deep call chains.
  - Svelte support as a plugin (`onus-plugin-svelte`); the LSP bridge waits for servers to load the project (tried with rust-analyzer and pyright); SCIP import tried with rust-analyzer and scip-typescript on real repositories.
  - Framework packs: YAML files of tree-sitter queries that add routes, events, data access and other relationships without code, from onus.yaml or the plugins file; Onus's own Prisma, `process.env`, HTTP-client and event extractors are now packs. Cargo and Python projects are discovered as components; a `container` sandbox preset; real sandbox tests in CI.
  - Pluggable map building (ADR 0006): discovery, language and fact providers; external plugins speaking a JSON protocol (`--plugins`); SCIP index import (`--scip`); an LSP bridge for any language server; trusted mode with a sandbox for tools that run repository code; the confidence `compiler`; a reference plugin.
  - `onus help [command | topic]` and a user guide (`docs/guide.md`) that ships in the binary: getting started, commands, reports, kinds of change, `onus.yaml`, the intent check, CI, how it works and troubleshooting. Every command's help has examples, and the commands that can fail on findings list their exit codes.
