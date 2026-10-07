# 0004. Additions to the map and report formats

Status: Proposed (2026-10-05)

## Context

`PLAN.md` section 4 sketches the map and report types. Building the first version showed that the diff engine and the report need a few facts the sketch does not have, and that some sketched fields need a different shape to stay deterministic.

## Decision

The JSON formats (generated into `schemas/`, `schemaVersion: 1`) add the following to the sketch:

- **`CodebaseMap.files`**: every analyzed file with a token hash that ignores formatting, comments and import paths, plus the resolved targets of its imports and re-exports. This is how Onus tells "reformatted" and "moved" apart from real edits without keeping the source.
- **`SymbolNode`**: a `variable` kind for top-level `let`/`var`; `name` (qualified, e.g. `EventBus.publish`); `facts` (comparisons, throws, guards, awaits and empty catches in a body, for notable edits); `literal` (the value of a string, number or boolean initializer, to resolve event names and spot changed constants; secrets are redacted); `componentId` is optional, because events, tables and config keys belong to the whole repository; `loc.signatureEnd`, so a contract row covers a function's signature and not its body.
- **`Component.packageName`**, and `PackageDep` gains `section`, `file` and `line` for evidence.
- **`TestNode`**: cases with their assertions (normalized text and line), markers, expected literal values and a body fingerprint; `literals` used in the file. Secrets are redacted from every stored text.
- **`SemanticChange`**: `component`, `kindLabel` (the Kind column), and two hints, `intentMismatch` and `needsPerson`. Each `location` has a `side` (`base` or `head`), because evidence for removed lines points into the base tree.
- **`SemanticReport`**: a `summary` with counts, `structure` (moved files and formatting-only files, which are not meaning rows), and `intentCheck.mismatches` as `{ changeId, reason }` instead of plain strings, so tools can join them to rows. Rule violations appear both in `changes` (ranked) and in `ruleViolations`.

Shapes of unannotated `const`s with a literal initializer record the widened type (`number`, `string`, `boolean`) as verified, not TypeScript's literal type; a changed value is reported as a changed constant instead of a contract change.

## Consequences

- Everything in a report can be traced to a map fact and a file location.
- The map is larger than the sketch (the shop fixture's base map is about 9,000 lines of JSON). Phase 2 moves it to SQLite behind the same types.
- Consumers should read the schemas, not the sketch in `PLAN.md`.
