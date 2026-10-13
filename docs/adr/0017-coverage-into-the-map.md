# 0017. Coverage into the map

Status: Accepted (2026-10-13)

## Context

The manifesto says the map holds "evidence links: which tests exercise which paths", that agents ask "which tests exercise the refund path?", and that the classifier weighs "evidence strength: how well tests and traces cover the changed paths". It also says "parts of the map with low confidence count as higher risk" and "never trust the author's test results".

Until now, a test was linked to the symbols its file refers to in its syntax. A test that imports `applyDiscount` was never linked to the private `noDiscount` it runs, and a test that imports a type or mocks a module was linked as if it ran the code. ADR 0014's `hints.tested` says whether any test reaches a changed file, not whether the changed lines ran. Nothing read coverage.

## Decision

1. **Coverage is a runtime fact of one tree, added after the map is built**, like traces (ADR 0010). It never changes a cached base map. `CodebaseMap.coverage` holds the sources, each measured file's covered and uncovered lines, links from tests to the symbols they ran, and notes on what was left out.
2. **Three formats, detected from the content**: lcov (Jest, Vitest, c8, nyc, cargo-llvm-cov, coverage.py, gcov), Cobertura XML and Istanbul JSON. Paths resolve to map files like trace paths do: the same path, or the longest map path an absolute CI path ends with. Coverage of test files is dropped; they are not the code under test.
3. **Head only.** Line numbers belong to the commit the tests ran at, so `--coverage` on `onus diff` and `onus report` applies to the head tree only. `--traces` applies to both.
4. **Two origins.** `supplied`: a file given with `--coverage` or listed in `tests.coverage`. `recorded`: a `coverage` artifact of a run an environment recorded (`environment.coverage`), read through `onus report --evidence` and when a submission is made without a report (`onus ci pr`, `onus_submit`). A recorded run counts only when its manifest is intact and it ran at the head commit. Each recorded source keeps the command of its run.
5. **Two guards.** Coverage naming a line past the end of a file was measured at another commit: that file's data from that source is left out, with a note. For the map server, a source file modified after the coverage file was written is left out, with a note to re-run the tests.
6. **Link strengths.** `onus_tests_for`, `onus query tests-for` and `onus_impact` say how each test link is known: `ran` (lcov `TN:` names the test running the target), `static-ran` (the test refers to it and the measured run executed it), `static` (no coverage for it) and `static-not-run` (it refers to it, but the measured run never executed it). Strongest first. Coverage never removes a static link: it only adds links and labels them.
7. **The report says which changed lines ran.** `SemanticReport.coverage` gives the changed executable lines, how many ran, the ranges no test ran, and the changed files with runtime code the coverage does not include. Rows get `hints.coverage`, and rows whose changed lines did not run rank first among equals. Without coverage, the JSON and Markdown are unchanged.
8. **A lane policy that only raises.** `lanes.inputs.coverage {minChanged, required}` is off unless written. Below `minChanged`, with a changed runtime file the coverage leaves out, or with no coverage when it is `required`, the change goes to the judge, with `source: "input"`. It never lowers a lane: a changed file the coverage does not measure counts as not run, so coverage of other files cannot let a change through that would not pass without coverage. The step runs only while the change would auto-merge.
9. **Recorded coverage counts for the lane only from named commands.** An intact manifest proves what a run recorded, not who chose the run. Over MCP the agent picks the command (`onus_env_run`), and a command can write a coverage file of its own. So the policy lists the commands whose recorded coverage counts, `lanes.inputs.coverage.commands`, compared with the manifest's command exactly. When the report holds recorded coverage from any other command, the change goes to the judge, whether or not coverage is `required`. The counts merge every source, so that coverage cannot be dropped alone, and treating the change as having none would let one extra run hide an honest run that tested too little. Supplied coverage counts: a person or CI gave it.

## Consequences

- The binary does all parsing, matching and counting. A hosted service hands it the runs it keeps (`onus report --evidence`) and stores what it wrote.
- Per-test links need lcov `TN:` records that name a test file. Aggregate coverage still confirms or weakens static links. Per-test data from coverage.py contexts or V8 dumps are later parsers.
- Branch and function coverage, coverage of the base, and a "coverage dropped" row are out of scope.
- Coverage measured at a checkout other than the head is left out by the guards or by the action, which passes `--coverage` only when the checkout is the head commit.
- A named command, or a CI step, still runs the change's own code, which could overwrite the coverage file before it is kept. Coverage is evidence the tests ran the lines, not proof; the judge and the weakened-tests floor stay the backstop.
