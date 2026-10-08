# 0010. Environments, the evidence store, traces, the production loop and contract-level scopes

Status: Accepted (2026-10-08)

## Context

Phase 5 closes the loop between what agents claim, what runs, and what happens in production (PLAN.md, Phase 5):

- disposable environments that produce evidence automatically;
- production outcomes feeding back into lanes;
- runtime traces adding what static analysis misses;
- scopes expressed in terms of components and contracts instead of paths.

Phases 3 and 4 already run commands in throwaway containers (`onus run-test`, the judge) and keep outcome records.

## Decision

1. **Environments** (`onus env create|run|list|destroy`). An environment is a container built from the repository at a commit and a declared environment: `environment:` in onus.yaml (image, setup, seed, evidence and trace globs), filled in from `.devcontainer/devcontainer.json` (its image and its create commands). The declaration is read from the operator's working tree, never from the commit under test, so a change cannot alter its own environment.
   - **Warm images:** setup runs once per image, setup command and lockfile contents, with network, and the container is committed as `onus-warm:<key>`, which later environments start from. Files of the commit the warm image was built from are removed before another commit's files are copied in, so deletions take effect.
   - **Seed data:** the seed command runs in each new environment after setup, so tests get synthetic data and never production data.
   - **Network:** none unless the task's token names hosts (`net:host:…`). Then the environment sits on an internal network whose only way out is an egress proxy container (squid by default, `environment.egressImage`) that allows exactly those hosts; `*.domain` allows a domain's subdomains, nothing broader is accepted.
   - **Secrets:** each `secret:NAME` the token names is passed from the operator's `ONUS_SECRET_NAME`, through the container engine's environment rather than its arguments; no other secret reaches the container. Secret values are replaced by `[secret NAME]` in everything the store keeps.
   - Every container runs without capabilities, with bounded memory, CPUs and processes, and gets the commit's files copied in as root's, never a host directory mounted. That also makes it work with engines that run in a VM (colima, podman machine); the test runner from ADR 0008 was changed to do the same.
2. **The evidence store** is content addressed, in the repository's git directory (`onus/evidence`): each run of a command in an environment leaves a manifest (commit, command, exit code, times, the environment with its hosts and secret names) named by the sha256 of its contents, and its artifacts under their sha256: the output log, the JUnit XML files the run wrote that match `environment.evidence` (their test counts are summed into the manifest), and the OpenTelemetry trace files that match `environment.traces`. `onus evidence list|show` reads it; an altered manifest or artifact no longer matches its hash and is refused.
   - Traces are collected as files (OTLP JSON, as the OpenTelemetry Collector's file exporter writes them) rather than by a receiver, because an environment has no network route to one.
   - `onus env run` prints a test run naming its manifest, ready for `onus submit --evidence`. The judge checks such a run against the store: the manifest must exist, be intact, have run at the head commit and record the same command and exit code. It still re-runs the command in a fresh container, with the environment's setup and seed.
3. **Traces feed the map** (`--traces <otlp.json>` on `onus map`, `diff` and `report`). Spans that carry `code.filepath` and `code.function` (and `code.namespace` for methods) resolve to symbols; a span's parent is its caller, so each resolved pair becomes a `calls` edge with confidence `traced`. A span of another service whose `service.name` is a component ties the caller to that component. Traced edges are added, never removed: a static edge no trace confirms stays. `diff` and `report` add the same traces to both maps, so a traced call is a fact of both trees rather than a change.
4. **The production loop** (`onus outcomes ingest-reverts|incident|backlog`) updates outcome records with what happened after merging: reverts found in git history (`This reverts commit …`) and incidents recorded against the change that caused them. A reverted change no longer counts toward its agent setup's record; a setup with an incident among its recent changes cannot auto-merge. The components and symbols incidents involved make a held-out backlog, most often first: where the next held-out tests should go. Onus does not write tests.
5. **Contract-level scopes.** A plan can name components and contracts instead of paths:
   - `writeComponents: [notifications.internal]`: the component's files that declare no public symbol; without `.internal`, all of its files.
   - `readContracts: [UserPreferences]`: the file that declares the contract's symbol.
   - `escalateBefore: ["contract:*"]`: the files that declare matching contracts get a `guard:path:` right. The gateway refuses a push that touches a guarded file unless the token also holds `unlock:path:` for it, which only an escalation grant adds; attenuation cannot.
   Minting resolves them through the map (`onus token mint --repo`) into path rights, so a token's paths do not drift while a task runs.

## Consequences

- Evidence stops being something an agent writes down and becomes something an environment records; the judge can rely on manifests rather than on claims.
- Containers remain the isolation boundary. MicroVMs (Firecracker) are left for where isolation demands them; the environment interface does not change.
- The egress proxy needs its image (`ubuntu/squid`) to be available to the engine; environments without hosts never start it.
- Programs must write traces and test results to files for them to become evidence; a run that writes none still leaves its log.
- Traced edges make the map more complete only where traces exist; a map built without traces is unchanged.
- Contract-level scopes are only as precise as the map; they are resolved when the token is minted, so a token's paths do not drift while a task runs.
