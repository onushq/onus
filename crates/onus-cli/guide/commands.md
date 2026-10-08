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
- `--plugins <file>`, `--trusted`, `--allow-unsandboxed`: see "Plugins" below.

## onus report --base <ref> --head <ref>

The same report for two git refs. Each ref is extracted with `git archive` into a temporary directory that is removed afterwards; the repository itself is never modified. Only the paths git reports as different are compared line by line, and the base tree takes the head's copy of every other file.

    onus report --base main --head HEAD
    onus report --repo ../shop --base origin/main --head feature/sms --format json

- `--repo <dir>`: the repository (default: the current directory).
- `--format`, `--markdown-out`, `--intent`, `--config`, `--fail-on`, `--traces`, `--plugins`, `--trusted`, `--allow-unsandboxed`: as for `onus diff`. SCIP indexes for `report` come from `scip` plugins, which index each ref in trusted mode.
- `--cache-dir <dir>`: keep the base ref's map in this folder and reuse it on the next report against the same base commit. The report is the same either way; see `onus help ci`.

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

Writes the JSON Schemas of the map, the report and onus.yaml.

    onus schema --out schemas
    onus schema                         # print all three

## onus mcp

Serves the map of the current worktree to a coding agent as MCP tools, on stdin and stdout. See `onus help agents` for the tools and how many agents share one map server.

    claude mcp add onus -- onus mcp

- `--repo <dir>`: the worktree (default: the one containing the current directory).
- `--no-server`: build the map in this process instead of the repository's shared map server.

## onus query <question>

The same questions as the MCP tools, from a shell, printed as JSON: `status`, `find <words>`, `symbol <id>`, `dependents <target>`, `dependencies <target>`, `tests-for <target>`, `impact <target> [--change <kind>]`, `invariants [target]`, `owners <target>`, `component <id>`, `file <path>`, `check [--base <ref>]` and `stats`.

    onus query find format phone
    onus query dependents UserPreferences --depth 2 --limit 20

- `--repo <dir>` and `--no-server`: as for `onus mcp`.
- `--depth <n>` (1 to 8) and `--limit <n>` (default 50, at most 500) where they apply.

## Scopes, lanes and environments

- `onus token`, `onus scope`, `onus gateway`, `onus escalate`, `onus escalation`, `onus audit`, `onus run-test`: task tokens, the git gateway, escalation and the audit log. See `onus help scopes`.
- `onus submit`, `onus classify`, `onus judge`, `onus outcomes`: risk lanes, the judge and outcome records. See `onus help lanes`.
- `onus env`, `onus evidence`: environments and the evidence store. See `onus help environments`.

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
