# 0010. The evidence factory, the production loop, traces and contract-level scopes

Status: Accepted (2026-10-08)

## Context

Phase 5 closes the loop between what agents claim, what runs, and what happens in production (PLAN.md, Phase 5):

- disposable environments that produce evidence automatically;
- production outcomes feeding back into lanes;
- runtime traces adding what static analysis misses;
- scopes expressed in terms of components and contracts instead of paths.

Phases 3 and 4 already run commands in throwaway containers (`onus run-test`, the judge) and keep outcome records.

## Decision

1. **Environments** (`onus env create|run|destroy|list`). An environment is a container built from the repository at a commit and a declared environment: `environment:` in onus.yaml (image, setup, seed, evidence globs), or `.devcontainer/devcontainer.json`'s image and post-create command.
   - **Warm pool:** setup runs once per image, setup command and lockfile contents, and is snapshotted (`docker commit`) as a warm image reused by later environments.
   - **Seed data:** the seed command runs after setup, in the environment, so tests get synthetic data and never production data.
   - **Network:** off unless the task's token names hosts (`net:host:…`). Then the container reaches the network only through an egress proxy Onus runs, which allows exactly those hosts.
   - **Secrets:** each `secret:NAME` the token names is passed from the operator's `ONUS_SECRET_NAME` variable; no other secret reaches the container.
2. **The evidence store** is content addressed: each run of a command in an environment leaves a manifest (environment, commit, command, exit code, times) and its artifacts under their sha256: the output log, JUnit XML files matched by the evidence globs, and OpenTelemetry traces. Traces arrive at a small OTLP/HTTP receiver (JSON) the run points the process at. `onus evidence show <run>` prints a manifest; a test run in a submission can name its manifest so the judge can compare.
3. **Traces feed the map** (`onus map --traces <otlp.json>` and the same option on `report`). Spans that carry `code.filepath` and `code.function` resolve to symbols; a span's parent is its caller, so each parent-child pair becomes a `calls` edge with confidence `traced`. Spans of different services become dependencies between their components (`service.name` names a component). Traced edges are added, never removed: a static edge that no trace confirms stays.
4. **The production loop** (`onus outcomes ingest-reverts`, `onus outcomes incident`) updates outcome records with what happened after merging: reverts found in git history (`This reverts commit …`, `Revert "…"`) and incidents recorded against the change that caused them. A reverted change no longer counts toward its agent setup's record; a setup with an incident in its recent record cannot auto-merge. Each incident adds the components and symbols it involved to a held-out backlog (`onus outcomes backlog`): where the next held-out tests should go. Onus does not write tests.
5. **Contract-level scopes.** A plan can name components and contracts instead of paths:
   - `writeComponents: [notifications.internal]`: the component's files that define no public symbol; without `.internal`, all of its files.
   - `readContracts: [UserPreferences]`: the files that declare the contract's symbol.
   - `escalateBefore: ["contract:*"]`: changing the file that declares a matching contract needs a granted escalation first.
   Minting resolves them through the map into path rights, and records what they came from in the token. The gateway's hook refuses a push that touches a contract file named by an `escalate-before` right unless the token also holds a write right for that file (which only an escalation grant adds).

## Consequences

- Evidence stops being something an agent writes down and becomes something an environment records; the judge can rely on manifests rather than on claims.
- Containers remain the isolation boundary. MicroVMs (Firecracker) are left for where isolation demands them; the environment interface does not change.
- Traced edges make the map more complete only where traces exist; a map built without traces is unchanged.
- Contract-level scopes are only as precise as the map; they are resolved when the token is minted, so a token's paths do not drift while a task runs.
