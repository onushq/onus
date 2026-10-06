# Commands

Every command prints to stdout; errors and warnings go to stderr. Run `onus help <command>` for the flags of one command.

## onus map <dir>

Builds the map of a directory and prints a summary.

    onus map .
    onus map . --json > map.json
    onus map . --config other/onus.yaml

- `--json`: print the whole map (format: schemas/codebase-map.schema.json).
- `--config <file>`: use this onus.yaml instead of `<dir>/onus.yaml`.

## onus diff <base> <head>

Reports the changes in meaning between two directories.

    onus diff old new
    onus diff old new --format json > report.json
    onus diff old new --intent pr-body.md --fail-on rule-violation,secrets

- `--format md|json`: Markdown (default) or the full JSON report (format: schemas/semantic-report.schema.json).
- `--intent <file>`: check the change against a stated intent: a YAML file, or Markdown (such as a pull request body) containing an `onus-intent` block. See `onus help intent`.
- `--config <file>`: the onus.yaml used for both trees. By default Onus uses the base tree's onus.yaml for both, so a change cannot relax the rules it is checked against.
- `--fail-on <what>`: exit with code 2 when `rule-violation` (a new boundary-rule violation) or `secrets` (a committed secret) is found. Repeat the flag or separate values with commas. `none` never fails.

## onus report --base <ref> --head <ref>

The same report for two git refs. Each ref is extracted with `git archive` into a temporary directory that is removed afterwards; the repository itself is never modified.

    onus report --base main --head HEAD
    onus report --repo ../shop --base origin/main --head feature/sms --format json

- `--repo <dir>`: the repository (default: the current directory).
- `--format`, `--intent`, `--config`, `--fail-on`: as for `onus diff`.

Any ref git understands works: branches, tags, `HEAD~3`, commit hashes. In JSON, `base` and `head` read like `main (abc1234)`.

## onus init [dir]

Writes a starter onus.yaml inferred from workspaces and CODEOWNERS. Sensitivity labels are only suggested, as comments, until you confirm them.

    onus init
    onus init path/to/repo --stdout     # print instead of writing
    onus init --force                   # overwrite an existing onus.yaml

## onus schema

Writes the JSON Schemas of the map, the report and onus.yaml.

    onus schema --out schemas
    onus schema                         # print all three

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
