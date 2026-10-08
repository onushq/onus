# How Onus works

    base tree ──► build map ──┐
                              ├──► semantic diff ──► ranked rows ──► Markdown / JSON
    head tree ──► build map ──┘         ▲
                                        └── line diff of the two trees

Onus builds a map of each version of the code and compares the maps, not the text. Every fact in a report comes from that analysis and a fixed sentence template; no language model writes any of it. Onus only reads files.

## 1. Building a map

Map building is a pipeline of providers: discovery providers find components, language providers analyze files, and fact providers add relationships. The ones below are built in; plugins add more, including SCIP indexes and language servers (`onus help plugins`).

**Files.** Onus walks the tree, skipping dependencies, build output and anything gitignored (see `onus help configuration`).

**Components** come from onus.yaml, else from Nx projects and npm, pnpm or yarn workspaces, else from the folders under services/, packages/ and apps/. Owners come from onus.yaml or CODEOWNERS. Each component's entrypoint comes from its package.json, mapped from build output back to source.

**Parsing.** Every .ts, .tsx, .js, .jsx, .mts and .cts file is parsed in parallel with tree-sitter. From each file Onus records:

- imports and exports, including aliases and re-exports;
- declarations: functions, interfaces, type aliases, classes, enums, constants and variables;
- each declaration's **shape**: parameters and which are optional, return types, fields and which are optional, all as normalized type text. A type that is inferred rather than written is marked unverified;
- each declaration's **fingerprint**: a hash of its code with comments, formatting, quote style, its own name and its local variable names removed. Two functions with the same logic under different names share a fingerprint;
- **body facts**: comparisons, guards, throws, awaits and empty catch blocks;
- **references**: calls and type uses, and which declaration they occur in;
- **patterns**: event publish and subscribe calls, Prisma reads and writes, `process.env` reads, calls into known SDKs, and `fetch`/`axios` calls to literal hosts;
- **tests**: cases with their full names, assertions, skip/only/todo markers and expected values.

**Linking.** Onus resolves every import itself, with no Node and no TypeScript compiler: relative paths, index files, tsconfig `paths` and `baseUrl`, and workspace package names mapped to their source. It follows re-export chains and respects local scope, so a call resolves to the declaration it really reaches. Imports of anything else become third-party packages.

**The map** gives every node a stable id: `billing:src/discount.ts#applyDiscount`, `event:OrderShipped`, `db-table:payment`, `config-key:SENDGRID_API_KEY`, `external:acme-sms`, `npm:date-fns`. Edges (`imports`, `calls`, `references-type`, `reads`, `writes`, `publishes`, `consumes`, `calls-external`, `reads-config`) connect them, each with the files and lines where it happens. Symbols reachable from an entrypoint are public; the rest are internal. The map also holds package dependencies, tests, boundary rules and diagnostics for anything Onus could not resolve. `onus map . --json` prints it.

## 2. The semantic diff

The passes run in this order:

1. **Matching.** Symbols with the same id are the same symbol. Of the rest, a removed and an added symbol with the same fingerprint are the same code: a move (same name, possibly into another component) or a rename (new name). A symbol with the same name and kind in another file whose members mostly overlap was moved and edited at once. Files that moved with identical content, or with only formatting and import-path changes, are matched too.
2. **Renames** become one row with the number of call sites updated; moves between components become one row per pair of components.
3. **Contracts.** Shapes of public symbols are compared, ignoring the order of object and union members: optional additions are additive; removals, new required members and unprovable type changes are breaking when a user or implementation outside the change was not updated.
4. **Boundary rules** from onus.yaml are checked on the new map; violations that already existed are dropped.
5. **Relationships** are compared per component: events published and consumed, tables read and written, external services called, and dependencies between components. A new vendor or a new kind of data leaving the system is security-sensitive.
6. **Packages.** New, changed and removed npm dependencies, and patched packages. An SDK of a newly reported external service is folded into that row.
7. **Risk classes.** Migrations, GraphQL operations, HTTP routes and authentication or authorization code get their own rows.
8. **Configuration** files are classified and the keys that changed are named; CI, policies, onus.yaml and CODEOWNERS are flagged as rules of the game, root build config and infrastructure as needing a person.
9. **Tests** are compared case by case for weakening; tests that moved, or went with the code they tested, are told apart.
10. **Notable edits** inside functions on both sides: flipped comparisons, changed constants, removed guards, throws and awaits (unless they moved into another function), swallowed errors.
11. **Secrets** in added lines.
12. **Generated code.** Rows about files a tool wrote become one row per component.
13. **Internal rows.** Every row lists the lines it explains; changed lines no row explains collapse into one internal row per component.

Then rows that say the same thing are grouped (`onus help changes`), hints are filled in (labels, blast radius, confidence), the intent check marks mismatches, and the rows are ranked (`onus help reports`).

## Why it is built this way

- **Facts are computed, never generated.** A wrong fact costs more trust than a missing one, so every row comes from deterministic analysis.
- **Conservative under uncertainty.** What Onus cannot resolve is marked low confidence and ranked as higher risk, never dropped.
- **Deterministic.** Everything is sorted; there are no timestamps or absolute paths. The same inputs give byte-identical output.
- **Safe.** Nothing from the analyzed code is installed or run, so Onus can run on any pull request.
- **Fast.** A generated workspace of about 230,000 lines maps in under a second on a laptop.

The design and its decisions are in PLAN.md and docs/adr in the repository.
