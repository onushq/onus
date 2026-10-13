# Environments and evidence

An environment is a container built from your repository at a commit. Commands run in it leave evidence (their output, test results, traces and coverage) in a content-addressed store, so a reviewer, or the judge, relies on what an environment recorded instead of what an agent says it ran. Environments need Docker or Podman.

## Declaring the environment

In onus.yaml:

    environment:
      image: node:22-alpine
      setup: npm ci                          # with network, once per lockfile
      seed: node scripts/seed-test-data.mjs  # synthetic data, in each new environment
      evidence: ["reports/**/*.xml"]         # JUnit XML the tests write
      traces: ["otel/*.json"]                # OTLP JSON trace files
      coverage: ["coverage/lcov.info"]       # lcov, Cobertura XML or Istanbul JSON

Without an `image`, Onus uses `.devcontainer/devcontainer.json`: its `image`, and its `onCreateCommand`, `updateContentCommand` and `postCreateCommand` as the setup. A devcontainer that builds from a Dockerfile needs the built image named in `environment.image`.

The declaration comes from your working tree (or `--config`), never from the commit under test: a change cannot give itself a different environment.

## Creating one

    onus env create --ref feature/sms --name sms
    onus env create --ref HEAD --token "$TOKEN" --key-public root.pub   # named after the token's task

1. **Warm image.** `setup` runs once per image, setup command and lockfile contents, and the result is kept as `onus-warm:<key>`. The next environment with the same lockfiles starts from it in about a second.
2. **The commit's files** are copied in. Files of the commit the warm image was built from are removed first, so deletions take effect.
3. **Seed.** `seed` runs, so tests get synthetic data, never production data.

The container has no capabilities, 4 GB of memory, 2 CPUs and at most 1024 processes.

**Network:** none. If the task's token names hosts (`net:host:api.example.com`, or `*.example.com` for subdomains), the environment sits on an internal network whose only way out is an egress proxy that allows exactly those hosts (`ubuntu/squid` by default; `environment.egressImage` to change it). `HTTP_PROXY` and `HTTPS_PROXY` point at it.

**Secrets:** each `secret:NAME` in the token is set as `NAME` from your `ONUS_SECRET_NAME` variable; creating fails if it is not set. No other secret reaches the container, and secret values are replaced by `[secret NAME]` in everything the store keeps.

## Running commands

    onus env run sms -- npm test
    onus env run sms -- npx vitest run --reporter=junit --outputFile=reports/unit.xml

Each run prints a test run naming its manifest, and exits 1 when the command failed. The output is ready for `onus submit --evidence`:

    onus env run sms -- npm test > run.json
    onus submit --base main --head feature/sms --evidence run.json --agent-tool claude-code --out submission.json

The judge checks every test run that names a manifest against the store: the manifest must be there, unaltered, from the head commit, with the same command and exit code. It still re-runs the command in a fresh container with the environment's setup and seed.

    onus env list                     # name, commit, state, image, hosts
    onus env destroy sms              # the container, its proxy and its network

## The evidence store

Runs are kept in the repository's git directory (`.git/onus/evidence`, shared by its worktrees; `--store` for another place):

    onus evidence list                               # id, commit, environment, exit code, command, test counts
    onus evidence show 3fa2c1d9                      # a manifest
    onus evidence show 3fa2c1d9 --artifact -         # its output log
    onus evidence show 3fa2c1d9 --artifact otel/trace.json > trace.json

A manifest records the commit, the command, the exit code, start and finish times, the environment (image, warm image, setup, seed, hosts, secret names) and its artifacts with their sha256: the log, the JUnit files the run wrote that match `evidence` (with test counts summed), the trace files that match `traces`, and the coverage files that match `coverage`. A manifest is named by the sha256 of its own contents, and artifacts by theirs, so an edited file no longer matches and is refused.

## Traces into the map

Traces show calls static analysis cannot see, such as an event bus dispatching to a handler, or a call into another service:

    onus map . --traces trace.json
    onus report --base main --head HEAD --traces trace.json

Spans with `code.filepath` and `code.function` (OpenTelemetry's code attributes) resolve to symbols, and each span's parent is its caller: the pair becomes a `calls` edge with confidence `traced`. A span of a service named like a component ties the caller to that component. Traced edges are only added; `diff` and `report` add them to both trees, so they never show up as changes. Files may hold one OTLP JSON document or one per line, as the OpenTelemetry Collector's file exporter writes them.

Production traces are kept with the records: `onus signals traces` turns an export into one `trace` signal in `signals.jsonl`, and `--traces` reads that file too, taking the calls of its `trace` lines and skipping the rest (`onus help production`). A call the map had only guessed (confidence `inferred` or `low`) that a trace observed is upgraded to `traced`; `onus map` says how many (`traces: 3 recorded calls, 2 traced calls added, 1 confirmed, 1 upgraded`). Static, compiler and declared edges stay as they are. With `records: true`, the action passes the records' `signals.jsonl` to `onus report --traces`.

## Coverage into the map

Coverage says which lines tests ran. Onus reads lcov (Jest, Vitest, c8, nyc, cargo-llvm-cov, `coverage lcov`), Cobertura XML (`coverage xml`, JaCoCo and .NET converters) and Istanbul JSON (`coverage-final.json`), and tells them apart by their content:

    onus map . --coverage coverage/lcov.info --json
    onus diff base/ head/ --coverage head-coverage.xml
    onus report --base main --head HEAD --coverage coverage/lcov.info --coverage py/coverage.xml

Coverage applies to the head only: its line numbers belong to the commit the tests ran at. Check out that commit before running the tests. Coverage that names a line past the end of a file was measured at another commit, and that file is left out with a note. Paths may be absolute (`/home/runner/work/shop/shop/src/a.ts`): the longest map path they end with counts. Coverage of test files is dropped.

The map keeps each measured file's covered and uncovered lines. An lcov `TN:` record that names a test file (`TN:services/billing/src/discount.test.ts`) also links that test to the symbols it ran (a bare file name links only when one test file has it), so `onus query tests-for noDiscount` finds a test that only imports `applyDiscount`. `onus map` says what it read (`coverage: 1 sources (lcov 1), 41 files matched, 3 not in the map; 1,204 of 1,530 lines ran; 18 test links`).

A run in an environment keeps the files that match `environment.coverage` as `coverage` artifacts. `onus report --evidence run.json` reads them, as long as the manifest is intact and the run was at the head commit; a run at another commit is skipped with a note, and so is a kept file that is not coverage, such as an HTML report the glob caught. They count as `recorded`; a file given with `--coverage` counts as `supplied`. Each recorded source keeps its run's command. A submission made without a report (`onus ci pr`, `onus_submit`) reads the recorded coverage of its evidence the same way:

    onus env run ci -- npm test -- --coverage
    onus report --base main --head HEAD --evidence run.json

An intact manifest proves what a run recorded, not who picked the command. An agent picks its own commands (`onus_env_run`), and one of them could write a coverage file. So the coverage lane policy counts recorded coverage only from the commands it names in `lanes.inputs.coverage.commands`, such as `npm test -- --coverage`, and sends a change with recorded coverage from any other command to the judge (`onus help lanes`). The command still runs the change's own code, so coverage is evidence that tests ran the lines, not proof.

For agents, `tests.coverage` in onus.yaml lists coverage files of the worktree (`onus help configuration`). The map server reads them on each rebuild and leaves out any source file edited after the coverage was written, with a note in `onus_status`.

## Behavior diff

`onus report --behavior` runs the tests that reach a change at the base and at the head, and reports which cases now behave differently (ADR 0019). It is off unless asked for, and needs a container engine. Declare the command in onus.yaml:

    environment:
      image: node:22-alpine
      setup: npm ci
      behavior:
        command: "mkdir -p reports && npx vitest run --reporter=junit --outputFile=reports/onus-behavior.xml {files}"
        junit: ["reports/onus-behavior*.xml"]   # the default
        budget: 600     # seconds for everything, environments included (default 600)
        maxFiles: 40    # most test files run (default 40)
        depth: 1        # code-dependency hops from changed code to tests, 0 to 2 (default 1)

`{files}` is replaced with the selected test files, shell-quoted and relative to the repository root. The command must write JUnit XML to a file `junit` matches. Like the rest of the report's config, it comes from the base commit (or `--config`), so a pull request cannot change the command that checks it.

    onus report --base main --head HEAD --behavior
    onus report --base main --head HEAD --behavior --behavior-budget 300 --fail-on behavior

1. **Selection.** From the maps: changed test files first, then the tests that exercise a changed file, then those that exercise code depending on it, up to `depth` hops. Both maps are asked, so code deleted at the head still selects its base tests. At most `maxFiles` run; the rest are counted as `truncated`. When nothing is selected, the report says "no tests reach the changed code", which is a fact worth knowing too.
2. **Base and head.** One environment per commit, made in parallel, each running the selected files that exist on its side.
3. **Retry.** Files with cases that fail at the head, and passed or did not exist at the base, run once more at the head. A case that passes then is flaky: no row, and it is listed in `behavior.flaky`.
4. **Cross.** The base versions of selected test files the change edited (and their changed `__snapshots__` files) are written over the head's tree and run against the head's code. Its manifest records this as `overlay: {fromCommit, paths}`, and the judge never accepts such a run as evidence of the head.
5. Both environments are destroyed, also after an error or a timeout.

Each case is compared, and becomes one of four rows, grouped per component (`onus help reports`): `behavior-newly-failing`, `behavior-changed`, `behavior-newly-passing` and `behavior-output-changed`. A case whose name and file are unknown to the maps counts under `root`. Cases failing at both sides with the same message are listed in `behavior.alreadyFailing`.

Every run is an ordinary evidence manifest, so `onus evidence show <id>` shows its log and JUnit. Each step gets what is left of the budget; one that would start after it is skipped, a run still going when it ends is stopped, and the status becomes `partial` with the reason. A newly failing case whose retry did not run still gives a row, at confidence `inferred`. Creating an environment is not stopped part way, so a slow setup can take the time past the budget. A problem never fails the report: without `environment.behavior`, `{files}` in the command, a container engine or an image, the status is `skipped` with the reason, and one `onus: behavior: …` line goes to stderr.

Limits: only runners that write JUnit and take test files as arguments; the cross run uses the head's versions of helpers and fixtures; a case renamed at the head pairs only by name, so its base and head runs are not compared (its cross run still counts); flakiness is ruled out with one re-run; and traces from these runs are not added to the map.
