# Running Onus in CI

The official GitHub Action (milestone M6) will download a release binary, post a sticky pull request comment and cache the base map. Until then, install from source and run `onus report` yourself. Onus only reads the checked-out files, so this is safe on pull requests from forks with read-only permissions.

## GitHub Actions

    name: Onus
    on: pull_request
    permissions:
      contents: read
    jobs:
      onus:
        runs-on: ubuntu-latest
        steps:
          - uses: actions/checkout@v5
            with:
              fetch-depth: 0          # onus report needs both commits
          - name: Install onus
            run: |
              rustup update stable && rustup default stable
              cargo install --git https://github.com/onushq/onus onus-cli --locked
          - name: Save the pull request body for the intent check
            env:
              BODY: ${{ github.event.pull_request.body }}
            run: printf '%s' "$BODY" > pr-body.md
          - name: Report
            env:
              BASE: ${{ github.event.pull_request.base.sha }}
              HEAD: ${{ github.event.pull_request.head.sha }}
            run: |
              onus report --base "$BASE" --head "$HEAD" --intent pr-body.md >> "$GITHUB_STEP_SUMMARY"
              onus report --base "$BASE" --head "$HEAD" --intent pr-body.md --format json > onus-report.json
          - uses: actions/upload-artifact@v4
            with:
              name: onus-report
              path: onus-report.json
          - name: Fail on new rule violations or secrets
            env:
              BASE: ${{ github.event.pull_request.base.sha }}
              HEAD: ${{ github.event.pull_request.head.sha }}
            run: onus report --base "$BASE" --head "$HEAD" --format json --fail-on rule-violation,secrets > /dev/null

Notes:

- The pull request body is passed through an environment variable, never pasted into the script, so a body cannot inject shell commands.
- Use the base and head commit hashes, not branch names: on `pull_request`, the checkout is a merge commit.
- Building Onus takes a few minutes; cache `~/.cargo/bin` with `actions/cache` if that matters to you.
- The Markdown starts with a hidden `<!-- onus-report -->` marker, so a bot can find and update its own comment.

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
