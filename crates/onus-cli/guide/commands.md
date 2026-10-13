# Commands

Every command prints to stdout; errors and warnings go to stderr. Run `onus help <command>` for the flags of one command.

## onus map <dir>

Builds the map of a directory and prints a summary.

    onus map .
    onus map . --json > map.json
    onus map . --config other/onus.yaml

- `--json`: print the whole map (format: schemas/codebase-map.schema.json).
- `--config <file>`: use this onus.yaml instead of `<dir>/onus.yaml`.
- `--scip <file>`: import a SCIP index of the directory (repeatable). Runs nothing.
- `--traces <file>`: add the calls in OpenTelemetry traces (OTLP JSON) as `traced` edges (repeatable). See `onus help environments`.
- `--coverage <file>`: add line coverage of the directory (lcov, Cobertura XML or Istanbul JSON), measured at the same version of its files (repeatable). See `onus help environments`.
- `--plugins <file>`, `--trusted`, `--allow-unsandboxed`: see "Plugins" below.

## onus diff <base> <head>

Reports the changes in meaning between two directories.

    onus diff old new
    onus diff old new --format json > report.json
    onus diff old new --intent pr-body.md --fail-on rule-violation,secrets

- `--format md|json`: Markdown (default) or the full JSON report (format: schemas/semantic-report.schema.json).
- `--markdown-out <file>`: also write the Markdown report to a file, so one run gives both formats.
- `--intent <file>`: check the change against a stated intent: a YAML file, or Markdown (such as a pull request body) containing an `onus-intent` block. See `onus help intent`.
- `--config <file>`: the onus.yaml used for both trees. By default Onus uses the base tree's onus.yaml for both, so a change cannot relax the rules it is checked against.
- `--fail-on <what>`: exit with code 2 when `rule-violation` (a new boundary-rule violation) or `secrets` (a committed secret) is found. Repeat the flag or separate values with commas. `none` never fails.
- `--base-scip <file>`, `--head-scip <file>`: import SCIP indexes of each tree (repeatable).
- `--traces <file>`: add the calls in OpenTelemetry traces to both trees (repeatable), so they are facts of both and never changes.
- `--coverage <file>`: line coverage of the head tree (lcov, Cobertura XML or Istanbul JSON), from tests run at the head (repeatable). The report then says which changed lines tests ran; see `onus help reports`.
- `--plugins <file>`, `--trusted`, `--allow-unsandboxed`: see "Plugins" below.

## onus report --base <ref> --head <ref>

The same report for two git refs. Each ref is extracted with `git archive` into a temporary directory that is removed afterwards; the repository itself is never modified. Only the paths git reports as different are compared line by line, and the base tree takes the head's copy of every other file.

    onus report --base main --head HEAD
    onus report --repo ../shop --base origin/main --head feature/sms --format json

- `--repo <dir>`: the repository (default: the current directory).
- `--format`, `--markdown-out`, `--intent`, `--config`, `--fail-on`, `--traces`, `--coverage`, `--plugins`, `--trusted`, `--allow-unsandboxed`: as for `onus diff`. SCIP indexes for `report` come from `scip` plugins, which index each ref in trusted mode.
- `--evidence <file>`: a test run from `onus env run` (repeatable). The coverage it recorded is used when its manifest is intact and it ran at the head commit; otherwise a note says why not.
- `--cache-dir <dir>`: keep the base ref's map in this folder and reuse it on the next report against the same base commit. The report is the same either way; see `onus help ci`.
- `--head-map-out <file>`: also write the head ref's map (schemas/codebase-map.schema.json) to a file. It is the map the report was computed against, so `onus query --map <file>` answers questions about the head with no second build. It holds the traces and coverage the report was given, so two reports of one commit with different evidence write different maps.

Any ref git understands works: branches, tags, `HEAD~3`, commit hashes. In JSON, `base` and `head` read like `main (abc1234)`.

## Plugins

- `--plugins <file>`: a plugins file listing language, framework, SCIP and LSP plugins (default: the file named by `ONUS_PLUGINS`). It is never read from the analyzed repository.
- `--trusted`: allow plugins that may run code from the repository (indexers, language servers). They run in a sandbox with no network.
- `--allow-unsandboxed`: with `--trusted`, allow them even where no sandbox is available.

See `onus help plugins`.

## onus init [dir]

Writes a starter onus.yaml inferred from workspaces and CODEOWNERS. Sensitivity labels are only suggested, as comments, until you confirm them.

    onus init
    onus init path/to/repo --stdout     # print instead of writing
    onus init --force                   # overwrite an existing onus.yaml

## onus schema

Writes the JSON Schemas of the map, the report, onus.yaml, the plugins file and protocol, the provenance record (`onus provenance`), and the map overview (`onus query overview`).

    onus schema --out schemas
    onus schema                         # print them all

## onus mcp

Serves the map of the current worktree to a coding agent as MCP tools, on stdin and stdout. See `onus help agents` for the tools and how many agents share one map server.

    claude mcp add onus -- onus mcp

- `--repo <dir>`: the worktree (default: the one containing the current directory).
- `--no-server`: build the map in this process instead of the repository's shared map server.
- `--token <token>` and `--key-public <file>`: the task token every call is checked against; `--key <file>`: the root private key, with which the server mints grants itself; `--trusted-repo <dir>`: a checkout whose files the agent cannot edit, from which the policy decides escalations when signed out; `--audit <file>`: the log; `--no-actions`: only the map tools.
- `--cloud-token`, `--cloud-repo`, `--cloud-ttl`: how the server signs in to Onus Cloud (`onus help cloud`).

## onus query <question>

The same questions as the MCP tools, from a shell, printed as JSON: `status`, `find <words>`, `symbol <id>`, `dependents <target>`, `dependencies <target>`, `tests-for <target>`, `impact <target> [--change <kind>]`, `invariants [target]`, `owners <target>`, `component <id>`, `file <path>`, `overview`, `check [--base <ref>]` and `stats`.

    onus query find format phone
    onus query dependents UserPreferences --depth 2 --limit 20
    onus query overview
    onus query --map map.json dependents money

- `--repo <dir>` and `--no-server`: as for `onus mcp`.
- `--depth <n>` (1 to 8) and `--limit <n>` (default 50, at most 500) where they apply.
- `--map <file>`: answer from a map JSON file instead of a worktree: one from `onus map --json` or `onus report --head-map-out`, or a map downloaded from Onus Cloud. `-` reads stdin, so `curl … | onus query --map - dependents money` works in scripts. A map of another schema version is refused. No map server is started, `check` and `stats` need a worktree, and `impact --change add-required-member` lists every file that names the type, since there is no source to read.
- `overview`: every component with its kind, roots, owners, labels, package, files, lines and first 50 public symbols, the components it uses and that use it (with edge counts), its external services and events; then the links between components, the events, the declared contracts and the rules (schemas/map-overview.schema.json). It is what `onus ui` draws and what Onus Cloud shows for a map it keeps.

## onus ui

Opens the web interface for the repository: everything below and above, in a browser, served on 127.0.0.1. See `onus help web-interface`.

    onus ui
    onus ui --repo ../shop --port 0 --no-open

- `--repo <dir>`, `--port <n>` (default 4387; 0 picks one), `--no-open`.
- `--outcomes <file>`, `--audit <file>`, `--escalations <dir>`, `--keys <dir>`: where outcome records, the audit log, escalation requests and the root key pair are kept.

## Scopes, lanes and environments

- `onus token`, `onus scope`, `onus gateway`, `onus escalate`, `onus escalation`, `onus audit`, `onus run-test`: task tokens, the git gateway, escalation and the audit log. See `onus help scopes`.
- `onus submit`, `onus classify`, `onus judge`, `onus outcomes`: risk lanes, the judge and outcome records. `onus outcomes audits` lists the merged changes sampled for a person to audit and the audit log; `onus outcomes audit` records an audit. `onus lanes train` fits the learned lane model to outcome records; it may only raise lanes. See `onus help lanes`.
- `onus env`, `onus evidence`: environments and the evidence store. See `onus help environments`.
- `onus signals ingest|traces|areas`: production signals kept with the records, production traces for the map, and each component's history. See `onus help production`.

## onus ci

What the GitHub Action runs to keep outcome records on a branch (`onus help ci`); usable from any CI system.

    onus ci records pull --dir records                       # the onus/records branch into a folder
    onus ci pr --base "$BASE" --head "$HEAD" --change acme/shop#7 --branch codex/sms --author mike \
      --body-file body.md --report report.json --records records --out out
    onus ci comment --change acme/shop#7 --body-file comment.md --author lead --association MEMBER --records records
    onus ci closed --change acme/shop#7 --merged --commit "$SHA" --merged-by mike --records records
    onus ci push --records records --since "$BEFORE"
    onus ci rollout --commit "$SHA" --percent 25 --metrics canary.json --records records
    onus ci rollback --change acme/shop#7 --reason "checkout 500s" --records records
    onus ci records push --dir records --message "acme/shop#7 closed"

- `ci pr` submits, classifies and (with `--judge`) judges the change, appends to `pulls.jsonl`, writes `submission.json`, `classification.json`, `judgment.json`, `provenance.json`, `rollout.json` and the comment section `lane.md` to `--out`, and prints `{lane, verdict, agent, record, review, rollout, risk}` (`record.recentRollbacks` counts its agent setup's recent rollbacks; `rollout` is the plan's strategy, steps, bake time and whether it rolls back automatically; `risk` is the learned model's, or null). `--evidence <file>` adds test runs; `--reviews <file>` gives the pull request's GitHub reviews, for the human lane's review (`onus help ci`); `--model <file>` uses that learned lane model instead of the one at the base commit (it still needs `lanes.learned`).
- `ci comment` acts on `/onus approve <row>`, `/onus audit ok|miss [note]` and `/onus incident <note> [involved: a, b]` from OWNER, MEMBER or COLLABORATOR, and prints `{command, ok, reply, rerun}`. It also answers `/onus caught <note>` (from anyone) and `/onus accuracy ok|wrong <row>|missed <note>` (write access) with what was said, `verdict`, `row`, `note` and `by`. Accuracy is never recorded. `caught` from OWNER, MEMBER or COLLABORATOR is recorded as `caught: true` on the change's outcome, or on its pull record while it is open, and its answer says so with `recorded`. `--report <file>` (the report's JSON) checks `wrong <row>` against the report's rows and turns a row number into its id.
- `ci closed` turns the pull request's latest record into an outcome, and samples a change approved without a person for an audit (`onus help lanes`). `--merged-by <login>` keeps who merged it on the outcome, as `@login`, for its provenance. `ci push` records reverts.
- `ci rollout` (`--change` or `--commit`) prints the change's rollout plan (for a change recorded before plans, from `lanes.rollout` in `--repo` or `--config`); with `--percent` (and `--metrics`, the stage's samples) it decides `promote`, `hold`, `rollback` or `complete`, records the stage, records a `rolled-back` outcome on `rollback`, and exits 0 for promote or complete, 2 for rollback and 3 for hold. `ci rollback` records a rollback decided elsewhere; `--planned` marks one done on purpose, which counts against no one. See `onus help production`.
- `ci records` takes `--remote` (default `origin`) and `--branch` (default `onus/records`).

## onus provenance

One record of why a change was allowed through: the intent, the agent and model, the token it held, the evidence, any escalations, the lane and the judge's steps, who or what approved it, and how it rolled out (`onus help provenance`, schemas/provenance.schema.json).

    onus provenance --change acme/shop#7 --records records --from out/provenance.json --out p.json
    onus provenance --submission-id 4fa91c
    onus provenance --verify p.json

- `--change <id>`: the change, as the records name it. Default: the change of `--from`, else the submission's head commit.
- `--records <dir>` (or `--outcomes <file>`): the pull and outcome records and production signals, for the stage, approver and rollout. `--signals <file>` reads the signals from another file.
- `--from <file>`: an earlier record of the change; its facts from while the change was open are kept.
- `--submission <file>` or `--submission-id <id>`, `--classification <file>`, `--judgment <file>`: what was handed in and decided. `--submission-id` reads `.onus/submissions` of `--repo`, with the judgment, audit log, escalations and outcomes beside it.
- `--token <token>` with `--key-public <file>`: a task token, verified (an error when it does not verify) and never kept; `--audit <file>`, `--escalations <dir>` and `--task <id>`: the door log, escalation requests, and whose door decisions to include.
- `--out <file>`: write it instead of printing it. `--verify <file>`: check a record's digest and schema version; exit 1 when they do not match.

## onus login, onus logout, onus cloud

Onus Cloud from the terminal (`onus help cloud`). `onus login` signs the CLI in as you (approve the code it shows in the browser); in CI, `ONUS_API_KEY` stands in for it, and a scoped token in `ONUS_CLOUD_TOKEN` stands in for both.

    onus login [--org acme] [--no-browser]
    onus cloud status
    onus cloud env create [--repo acme/shop] [--ref pull/42] [--name sms] [--host registry.npmjs.org]
    onus cloud env run sms -- npm test
    onus cloud evidence acme/shop#42 --run <run id>
    onus cloud pulls --attention
    onus cloud token create --scope pulls:read [--repo acme/shop | --all-repos] [--ttl 2h] [--label sms]
    onus cloud map dependents UserPreferences [--repo acme/shop] [--commit <sha> | --pull 42]
    onus cloud escalate --task sms --scope 'write:path:services/billing/**' --evidence … --reason …
    onus cloud blocked --kind broken-test --evidence failing-test:src/a.test.ts --reason … --task sms
    onus cloud escalation <id> [--wait 600]
    onus logout

- `cloud env create|run|list|show|log|destroy`: hosted environments; `create` and `run` wait unless `--no-wait`, and `run` exits 1 when the command failed. `--json` prints the API's answer.
- `cloud pulls`, `cloud pull <owner/name#n>`: governed pull requests, their lanes, analyses and evidence.
- `cloud evidence <owner/name#n> --run <id>`: a recorded run to a pull request, for the judge.
- `cloud token create|list|revoke`: scoped tokens; `create` prints the token alone on stdout.
- `cloud map <question> | --download <file>`: `onus query`'s questions asked of the map Onus Cloud keeps, answered here.
- `cloud escalate`, `cloud blocked`, `cloud escalation <id>|list|withdraw`: escalations and blocked reports in Onus Cloud. `--token`, `--key-public` and `--key` are the door's; with `--key` a grant is minted here. Exit 0 when granted or answered, 1 when denied, withdrawn, expired or failed, 3 while waiting.
- `--org <slug>` on any `cloud` command, or `ONUS_CLOUD_ORG`, picks the organisation; `ONUS_CLOUD_URL` the address.

## onus help [command | topic]

    onus help                 # commands and guide topics
    onus help diff            # the flags of one command
    onus help configuration   # a guide topic

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success, including reports that contain findings |
| 1 | A usage error (unknown flag, missing argument) or a runtime error (unreadable directory, unknown git ref, invalid onus.yaml) |
| 2 | A `--fail-on` condition was met |

Exit code 2 is never used for errors, so CI can tell "Onus found something" from "Onus could not run".
