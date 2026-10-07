# 0005. Phase 1 semantic diff behavior

Status: Proposed (2026-10-05)

## Context

`PLAN.md` section 5 describes the semantic diff. Implementing it against the golden scenarios S1–S9 forced several choices the plan leaves open or states differently.

## Decision

1. **Module resolution lives in the TypeScript adapter** (`onus-lang-ts`), not in `onus-map`. Relative paths, `index` files, `tsconfig` `paths`/`baseUrl` and `.js`-to-`.ts` mapping are language rules. `onus-map` discovers workspace packages and their source entrypoints and hands them to the adapter.
2. **Both maps are built with the base tree's `onus.yaml`** (or `--config`). A pull request cannot relax the rules it is checked against; a changed `onus.yaml` is reported as a rules-of-the-game row instead.
3. **Relationships are compared per component**, not per symbol: "notifications consumes `OrderShipped`", "billing writes `payment`", "billing depends on notifications". Moving code inside a component never looks like a new relationship.
4. **Moves and formatting are structure, not meaning.** A file or symbol that moved with the same body, and a file whose tokens and resolved imports are unchanged, produce no row; they are listed under "Structure only". A *rename* (same body, new name) produces one internal row with the number of call sites updated. Rename detection uses exact body fingerprints; the plan's "≥ 0.9 similar" matching is not implemented yet.
5. **One meaning, one row.** A new npm package that is the SDK of a new external service in the same component is folded into the external-service row. Several new exports of one component are one row. Everything not explained by another row collapses into one internal row per component, counted from the lines no other row covers.
6. **Sensitivity.** New vendors, new data-egress categories, committed secrets, and new data writes or notable edits inside a component with a high-sensitivity label are `security-sensitive`. An additive change to a declared contract does not by itself need a person (the manifesto's example shows it as an open circle); breaking changes, rule violations, rules-of-the-game rows and intent mismatches do.
7. **Guards and throws are compared by count.** An edited guard condition is not reported as a removed guard; only fewer guards or throws are.
8. **"Source special-cases test literals"** only fires when the literal appears nowhere else in the component's source, so domain values that tests also use (`"invalid_phone"`) are not flagged.
9. **Changed constants are notable edits.** A `const` or variable whose literal value changes (`LOYALTY_RATE` from `0.1` to `0.15`) gets its own row, security-sensitive in a labeled component. Calls on a class field resolve through the field's declared type, so an SDK client held in `this.provider` is still an external call.
10. **Blast radius** counts every file that depends on the subject in the head map, tests included, except the file that defines it.
11. **Exit codes.** `0` on success, `1` for usage and runtime errors, `2` only when a `--fail-on` condition is met. (clap's default usage exit code, 2, is remapped so CI can tell the two apart.)
12. **Line counts come from an in-process text diff** (the `similar` crate, Myers) of the two trees rather than `git diff --numstat`, so `onus diff` works on plain directories. `onus report` extracts each ref with `git archive` into a temporary directory that is removed afterwards, instead of using worktrees, so the repository's metadata is never touched.
13. **Crate versions.** `tree-sitter` 0.27 with `tree-sitter-typescript` 0.23.2 (they are compatible through `tree-sitter-language`).

## Amendments after the first field run (2026-10-06)

Running Onus on a large Nx and pnpm monorepo (about 8,500 files, 300 projects) led to these changes:

- **Nx projects are components.** With an `nx.json` at the root, every `project.json` folder is a component; workspace packages remain, and the most specific component owns each file.
- **Moves between components are one row**, including symbols edited while moving (same name and kind, at least half their members in common, unambiguous). Their contract changes are folded into that row.
- **Grouping.** Config files of one kind per component, dependencies already in the repository per component, and any config or dependency change spread over more than 5 components are single rows; beyond 10 internal rows, the rest are summarized. Every location stays in the evidence.
- **pnpm `catalog:` versions are resolved** before dependencies are compared.
- **Tests do not create relationships**: events, data access, config reads and external calls are only taken from non-test code, and loopback, private, `.local`, `.internal` and `example.*` hosts are never external services.
- **Imports of files Onus does not analyze** (`.svelte`, `.graphql`, `.json`, ...) resolve as assets, and SvelteKit's `$lib` alias is understood.

## Consequences

- All nine golden scenarios produce the expected rows and nothing else.
- Exact-fingerprint renames miss renames that also edit the body; those show as a removed and an added symbol (and, for public symbols, as a breaking export removal plus an addition). A similarity measure is a follow-up.
- Per-component relationship diffs hide a second, new caller of an already-used dependency inside the same component. That is intended: it is not a new relationship.
