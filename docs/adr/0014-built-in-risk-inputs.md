# 0014. Built-in risk inputs that only raise a lane

Status: Accepted (2026-10-13)

## Context

The classifier routed a change by its rows' kinds and subkinds, the policy's rules and a few hard floors (ADR 0009). Several things the manifesto names as risk were computed but never read, or not computed at all: how many other components depend on what changed, whether tests or traces reach the changed code, rows that rest on parts of the map Onus could not resolve, and the reverts and incidents of the area being changed. A new npm package could auto-merge under a `match: every` rule, and an edit to the body of a contract declared in onus.yaml went through like any other code.

## Decision

1. **Four built-in inputs, on by default** (`lanes.inputs`): blast radius (other components that depend on a row, `{ judge: 3 }`), tests (a row whose code no test exercises and no trace has observed, `judge`), confidence (a row on unresolved parts of the map, `judge`) and history (reverts, rollbacks and incidents per component in the last 90 days, `{ judge: 1 }` and `{ human: 1 }`). Each is configurable or can be turned off. They run after the policy's rules and before the hard floors, the record and the audit, and **they only raise a lane**. Each one that fires is listed with `source: "input"`, a reason and the rows behind it. Blast radius counts the users of a row's subject, or, for a component's internal row, of the functions and classes whose lines changed, so a body-only edit of a widely used function counts.
2. **Three new hard floors** no policy can lower: third-party code new to the repository goes to a person; a package new to a component, or a new dependency between components, never auto-merges; an edit to the code of a declared contract is a rule of the game (a new `declared-contract-edited` row when no other row is about it), so it goes to a person.
3. **Test reach is per file.** A file is tested when a test in the head map exercises anything in it, and traced when a runtime trace saw it. It is cheap, stable and needs no test run. It is not line coverage, which comes from running tests and is a separate fact.
4. **History comes from the outcome records**, which now carry `components`: CI fills them from the report when the pull request is classified, and outcomes, reverts and incidents keep them. Every record in the window counts, a change once per component and kind. Records shared across repositories are scoped by the `owner/repo` of the change being classified, and the change itself is left out.
5. Every fact is computed by the binary: the hints by the diff, the history by `onus-lanes`, the lane by `classify`. A hosted service only stores the records and passes the policy through.

## Consequences

- More changes leave `auto-merge` than before: a policy that auto-merged everything in a component now also needs tests that reach it, a contained blast radius and a quiet history. Setting an input to `auto-merge` or `{}` restores the old behaviour for that input; the floors stay.
- Reports gain `hints.dependentComponents` and `hints.tested`; old reports read as 0 and absent, so the inputs never fire on them.
- File-level reach can call a file tested when the test touches only part of it. Line coverage, when recorded, is the finer fact.
- A repository can weaken its own inputs. An organisation-wide policy that forces them on is left for later; the hard floors already apply everywhere.
