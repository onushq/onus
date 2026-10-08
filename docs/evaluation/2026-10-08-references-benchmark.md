# How complete are the map's dependents? (2026-10-08)

Phase 2's gate asks whether the map's answers can be trusted: a benchmark with ground truth from the TypeScript compiler, measuring precision and recall per query type. `onus_impact`, `onus_dependents` and the breaking-change rows of every report rest on one fact, "these files depend on this symbol", so that is what this benchmark measures.

## Method

`scripts/benchmark-references.mjs` takes a repository and its map (`onus map <repo> --json`). For a deterministic sample of public symbols, spread evenly over kinds, it compares two sets of files:

- **Onus:** files with an `imports`, `calls` or `references-type` edge to the symbol or one of its members, outside the symbol's own file. This is `onus query dependents <symbol>` at depth 1.
- **TypeScript:** files where the TypeScript 5.9 language service finds references to the symbol (`findReferences`), outside its own file.

The language service runs over every source file of the repository, with the root tsconfig. The repositories have no installed dependencies, so the workspace's own packages are resolved to their sources through `paths` built from the map. TypeScript is loaded from outside the measured repository, and nothing in the repository runs.

Precision is the share of Onus's files that TypeScript also finds; recall is the share of TypeScript's files that Onus finds.

## Results

| Repository | Symbols | Files (TypeScript) | Precision | Recall |
|---|---|---|---|---|
| `fixtures/shop` (every public symbol) | 30 | 100 | 1.000 | 1.000 |
| Twenty, 31,000 files (sample of 100) | 100 | 2,584 | 0.923 | 0.999 |

On Twenty, per kind:

| Kind | Symbols | Precision | Recall |
|---|---|---|---|
| class | 17 | 0.888 | 0.995 |
| const | 17 | 0.829 | 1.000 |
| enum | 17 | 0.947 | 1.000 |
| function | 17 | 0.906 | 1.000 |
| type | 16 | 0.723 | 1.000 |
| interface | 16 | (no references outside their files in either set) | |

## What the differences are

- **Missed (3 files in 2,584):** uses of a generated client from test files of a separate app that imports it through a path the map does not resolve.
- **Extra (214 files):** mostly barrel modules that re-export the symbol (`export * from './events'`, `export type { X } from …`). The compiler does not count a re-export as a reference; Onus does, because a barrel breaks when the symbol is removed or renamed and is where many callers get it from. The rest are files that use a member through a namespace or a type that the language service attributes to the member's own declaration.

For `onus_impact` and breaking-change rows, missing a dependent is the costly error (a change looks safe when it is not); an extra barrel file costs a reviewer a glance. Recall is the number that matters, and it is above 99.8%.

## Limits

- One TypeScript monorepo besides the fixture. The benchmark script runs on any TypeScript repository; adding one is one command.
- Ground truth is the compiler's references, not people's judgment of what "depends on" means; dynamic uses (dependency injection by token, string lookups) are invisible to both.
- Symbol sampling is deterministic (`--seed`), so reruns compare like with like.
