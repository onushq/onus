# Environments and evidence

An environment is a container built from your repository at a commit. Commands run in it leave evidence (their output, test results and traces) in a content-addressed store, so a reviewer, or the judge, relies on what an environment recorded instead of what an agent says it ran. Environments need Docker or Podman.

## Declaring the environment

In onus.yaml:

    environment:
      image: node:22-alpine
      setup: npm ci                          # with network, once per lockfile
      seed: node scripts/seed-test-data.mjs  # synthetic data, in each new environment
      evidence: ["reports/**/*.xml"]         # JUnit XML the tests write
      traces: ["otel/*.json"]                # OTLP JSON trace files

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

A manifest records the commit, the command, the exit code, start and finish times, the environment (image, warm image, setup, seed, hosts, secret names) and its artifacts with their sha256: the log, the JUnit files the run wrote that match `evidence` (with test counts summed), and the trace files that match `traces`. A manifest is named by the sha256 of its own contents, and artifacts by theirs, so an edited file no longer matches and is refused.

## Traces into the map

Traces show calls static analysis cannot see, such as an event bus dispatching to a handler, or a call into another service:

    onus map . --traces trace.json
    onus report --base main --head HEAD --traces trace.json

Spans with `code.filepath` and `code.function` (OpenTelemetry's code attributes) resolve to symbols, and each span's parent is its caller: the pair becomes a `calls` edge with confidence `traced`. A span of a service named like a component ties the caller to that component. Traced edges are only added; `diff` and `report` add them to both trees, so they never show up as changes. Files may hold one OTLP JSON document or one per line, as the OpenTelemetry Collector's file exporter writes them.
