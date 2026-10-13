# 0019. A behavior diff: the tests that reach a change, run at the base and at the head

Status: Accepted (2026-10-13)

## Context

The manifesto describes four levels of a change. The fourth is behavior: "which inputs now produce different outputs, as shown by tests and traces". It tells the judge to check that tests weren't weakened, and warns that agents learn to satisfy tests rather than intent.

Every behavior-level row so far was a static heuristic: removed assertions, skip markers, edited expected values, boundary conditions. Nothing ran the tests at both commits. When an agent changes `>` to `>=` and edits the test to expect the new value, every test passes at the head, and the only signal was the static note "expected values edited". The judge re-runs the author's own runs at the head; it never compares them with the base.

Onus already had what is needed to run tests: environments built from a commit, and an evidence store that keeps each run's manifest, log and JUnit (ADR 0010). The map links tests to the code they exercise (ADR 0004), and coverage strengthens those links (ADR 0017).

## Decision

1. **Opt-in.** `onus report --behavior` runs it; `onus diff` cannot, since it has no commits. Without the flag the report is byte-identical to before. The cloud worker does not pass it yet.
2. **The config comes from the base commit, or `--config`** (ADR 0005), like the rest of the report's config: `environment.behavior {command, junit, budget, maxFiles, depth}`. `{files}` in the command is replaced with the selected test files. A pull request cannot change the command that checks it. The environment's image and setup also resolve against the base tree.
3. **The maps choose the tests.** `MapIndex::tests_reaching` gives changed test files first, then tests that exercise a changed file, then tests of code that depends on it, up to `depth` code-dependency hops (at most 2). Both maps are asked, so deleted code still selects its base tests. At most `maxFiles` run. An empty selection is reported as "no tests reach the changed code".
4. **Three runs and a retry.** The base's tests at the base and the head's tests at the head, each in its own environment, in parallel. Head files with cases that newly fail run once more at the head: a case that passes then is flaky, listed and given no row. Then the base versions of the selected test files the change edited (with their changed snapshot files) are written over the head's tree and run there, the cross run. Its manifest records `overlay: {fromCommit, paths}`, and the judge never accepts an overlaid run as evidence. Every run is an ordinary evidence manifest. Both environments are destroyed afterwards, also after an error or a timeout.
5. **One budget.** `budget` seconds cover everything. Each run gets what is left; a step that would start after it is skipped, and a run going past it is stopped. The status says `partial` and why. Creating an environment is not interrupted.
6. **Four rows, grouped per component**, like `test-weakened`: `behavior-newly-failing` (passes at the base, fails at the head and on the retry, or a new case that fails), `behavior-changed` (the base version of an edited test fails against the head's code while the head's version passes: the test was edited to match), `behavior-newly-passing` and `behavior-output-changed` (fails at both with a different normalised message). Cases are attributed to test files by the JUnit `file` attribute, a `classname` that is a selected file, or a unique case name in the maps. Rows use fixed templates and quote the failure message. A case that disappears at the head stays with `test-weakened`.
7. **Confidence `traced`.** The rows are observed at runtime. A newly failing case that could not be run again keeps its row at `inferred`, saying it was not re-run.
8. **Floors.** `behavior-newly-failing` is a new hard floor: blocked unless a person approves that row (`/onus approve <row id>`). `behavior-changed` is a rule of the game, so the existing floor sends it to a person. `--fail-on behavior` exits 2 on a newly failing row.
9. **A problem never fails the report.** No `environment.behavior`, no `{files}`, no container engine, no image, a failed setup, a run that wrote no JUnit, or the budget: the report is printed with `behavior.status` `skipped` or `partial` and a reason, and one `onus: behavior: …` line on stderr.

## Consequences

- Onus now runs the repository's tests on its own, in an environment with no network. The setup step still has network, as for every environment.
- The diff takes as long as the selected tests twice, plus a retry and a cross run when needed. `maxFiles`, `depth` and `budget` bound it.
- Only runners that write JUnit and take test files as arguments work. Node's built-in runner, vitest and jest (with jest-junit) do.
- The cross run uses the head's versions of helpers and fixtures. A test whose helper changed with it may fail for that reason, and be reported as changed behavior.
- A case renamed at the head pairs only by name, so its base and head runs are not compared; its cross run still counts.
- Flakiness is ruled out with one re-run only.
- Traces these runs could record are not added to the map yet; a later step could feed them to `add_traces`.
- Onus Cloud displays the rows like any other, from `report.json`. Running repository tests in the hosted analysis sandbox is a separate trust decision that belongs with hosted environments; a later policy flag can pass `--behavior`.
