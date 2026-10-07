# Running Onus in CI

Onus only reads the checked-out files, so it is safe on pull requests from forks with read-only permissions.

## GitHub Actions

The Onus action downloads the release binary for the runner, checks its SHA-256, writes the report to the job summary and posts it as one pull request comment that is updated on each push:

    name: Onus
    on: pull_request
    permissions:
      contents: read
      pull-requests: write      # for the comment; leave it out to skip it
    jobs:
      onus:
        runs-on: ubuntu-latest
        steps:
          - uses: actions/checkout@v7
            with:
              fetch-depth: 0
          - uses: onushq/onus/action@v0.1.0
            id: onus
            with:
              fail-on: rule-violation,secrets
          - uses: actions/upload-artifact@v7
            with:
              name: onus-report
              path: ${{ steps.onus.outputs.report }}

Inputs, all optional:

- `version`: the release to download, such as v0.1.0. Defaults to the tag the action is used at, otherwise the latest release.
- `onus-path`: use this binary instead of downloading one (for example one you built).
- `base`, `head`: the commits to compare. Default: the pull request's base and head commits.
- `intent`: `true` (the default) checks the change against an `onus-intent` block in the pull request body; `false` skips the check; a file path uses that file.
- `fail-on`: fail the job on `rule-violation`, `secrets` or both (comma separated). Default `none`.
- `comment`: `false` to only write the job summary.
- `config`: an onus.yaml to use for both commits.
- `working-directory`: the repository, if not the workspace root.
- `github-token`: the token for the download and the comment. Default: the workflow's token.

Outputs: `report` (path of the JSON report), `markdown` (path of the Markdown report), `meaning-changes` and `needs-attention` (counts from the report's summary).

Notes:

- The pull request body is passed through an environment variable, never pasted into a script, so a body cannot inject shell commands.
- The action compares the base and head commits, not branch names: on `pull_request`, the checkout is a merge commit. Commits missing from a shallow checkout are fetched.
- On pull requests from forks, GitHub gives the workflow a read-only token, so the action skips the comment and only writes the job summary.
- The comment starts with a hidden `<!-- onus-report -->` marker, which is how the action finds its own comment to update.
- The action runs on Linux (x86_64 and ARM64), macOS and Windows runners.

## Without the action

Install the binary and run `onus report` yourself:

    curl -fsSL https://onushq.com/install.sh | sh
    onus report --base "$BASE" --head "$HEAD" --intent pr-body.md >> "$GITHUB_STEP_SUMMARY"
    onus report --base "$BASE" --head "$HEAD" --format json --fail-on secrets > onus-report.json

## Other CI systems

Onus needs only git, the checkout and the binary. In GitLab merge request pipelines:

    onus report --base "$CI_MERGE_REQUEST_DIFF_BASE_SHA" --head "$CI_COMMIT_SHA" --fail-on secrets

## Using the exit code

- `0`: the report ran (it may still contain findings; read `summary` in the JSON).
- `1`: Onus could not run: a missing ref (fetch more history), an invalid onus.yaml, an unreadable directory.
- `2`: a `--fail-on` condition was met.

To branch on other findings, read the JSON, for example with jq:

    jq '.summary.needsAttention' onus-report.json
    jq -r '.changes[] | select(.hints.needsPerson) | .title' onus-report.json
