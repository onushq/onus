# 0006. Pluggable map building, language-author tooling and trusted mode

Status: Proposed (2026-10-06)

## Context

Onus should support any language or framework, and the most precise facts about a language come from tools written by its authors: compilers, language servers and the indexers built on them. Until now the map was built by one compiled-in TypeScript adapter on tree-sitter (ADR 0002), with framework knowledge (events, Prisma, SDKs) and component discovery (Nx, workspaces) hard-wired. Language servers and indexers, however, often run code from the analyzed repository (rust-analyzer runs build scripts and proc macros, gopls runs `go list`, tsserver loads tsconfig plugins, Gradle and Maven run build scripts) and need dependencies installed, which conflicts with the Phase 1 principle that Onus never runs analyzed code and is safe on fork pull requests.

## Decision

1. **Three plugin points.** Map building is a pipeline of *providers*:
   - *discovery* providers find components (Nx projects, package workspaces, Cargo, Go modules, Bazel, ...);
   - *language* providers turn source files into symbols, shapes, references and tests;
   - *fact* providers add relationships on top (events, data access, HTTP routes, external services).
   Built-in providers implement the same interfaces as plugins. The map records which providers built it.
2. **Plugins are external processes.** Onus starts the plugin's command, writes one JSON request to its stdin and reads one JSON response from its stdout. Requests and responses are versioned and published as `schemas/plugin-protocol.schema.json`; responses use the map's own types. Plugins can be written in any language and wrap any tool.
3. **Plugins are enabled by whoever runs Onus**, through a plugins file (`--plugins` or `ONUS_PLUGINS`), never because the analyzed repository's `onus.yaml` lists them: a pull request must not be able to make CI run new code.
4. **Language-author tooling, SCIP first.** Onus imports SCIP indexes (rust-analyzer, scip-typescript, scip-java, scip-python, scip-go), which are built for whole-repository indexing in one pass. A generic LSP bridge covers languages without an indexer. Facts confirmed by a compiler or indexer get the confidence `compiler`, which ranks above `static` (read from syntax).
5. **Executing tools are opt-in and sandboxed.** A provider that may run repository code (most indexers and language servers) declares it, and runs only in *trusted mode* (`--trusted`), for example on the main branch, and only inside a sandbox with no network (bubblewrap on Linux, `sandbox-exec` on macOS, or a configured container command). Running unsandboxed needs an explicit `--allow-unsandboxed`. Without trusted mode such providers are skipped and the map says so. Importing an index produced elsewhere (`--scip index.scip`) runs nothing and is always allowed.
6. **tree-sitter stays the default.** It needs no runtime, no installs and no network, so `onus report` on a fork pull request keeps working exactly as before.

## Alternatives considered

- **WebAssembly plugins**: sandboxed even when supplied by the repository, but harder to write and unable to drive native tools such as language servers.
- **Rust crates only**: simplest, but adding a language would mean rebuilding Onus.
- **LSP only**: broadest coverage, but language servers answer one editor request at a time; mapping tens of thousands of symbols through them is slow. Existing bridges that turn a language server into a SCIP index (for example `lsp-to-scip`) can be used as indexers through the plugin protocol.

## Consequences

- New languages and frameworks need no change to Onus.
- Reports can carry compiler-confirmed facts where a trusted run is possible, and stay safe everywhere else.
- Plugin output is untrusted input: Onus validates paths and ids, sorts everything for determinism, and enforces timeouts.
- The sandbox depends on the platform; where none is available, trusted runs need an explicit opt-out.
