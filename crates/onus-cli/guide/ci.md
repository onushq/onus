# Running Onus in CI

Onus only reads the checked-out files, so it is safe on pull requests from forks with read-only permissions. The one thing it writes, with `records: true`, is its own `onus/records` branch.

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
          - uses: onushq/onus/action@v0.10.0
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
- `fail-on`: fail the job on `rule-violation`, `secrets`, `behavior` (with `behavior: true`), or several (comma separated). Default `none`.
- `comment`: `false` to only write the job summary.
- `config`: an onus.yaml to use for both commits.
- `lanes`: `true` to classify the change into a risk lane with the policy in onus.yaml and the hard floors, and show it in the comment (`onus help lanes`). Output `lane`.
- `apply-lane`: `true` (with `lanes`) to label the pull request `onus/lane:<lane>` and fail the job for the `blocked` lane. Needs `issues: write`.
- `wait-for-review`: `true` (the default, with `records`) fails the job while the change is in the human lane and no review counts yet. Output `review`: `waiting` or `approved`. See "Reviews in the human lane" below.
- `submission`: (with `records`) the agent's own submission of the change, for its provenance. When empty, one kept on the records branch is used. See "The agent's submission" below.
- `auto-merge`: `true` (with `lanes`) to enable GitHub's auto-merge (squash) for the `auto-merge` lane. Required checks still apply. Needs `contents: write`.
- `plugins`: a plugins file's contents (YAML), written outside the repository and passed with `--plugins`. For Svelte components: `plugins: [{ name: svelte, kind: language, command: [onus-plugin-svelte], files: ["**/*.svelte"] }]` (the plugin ships in every release archive).
- `working-directory`: the repository, if not the workspace root.
- `coverage`: coverage of the head commit (lcov, Cobertura XML or Istanbul JSON) written by an earlier step, as a glob. Passed only when the checkout is the head commit (see "Which changed lines tests ran" below).
- `behavior`: `true` to run the behavior diff (`onus report --behavior`): the tests that reach the change run at the base and at the head in containers, from `environment.behavior` in the base commit's onus.yaml (`onus help environments`). GitHub's Linux runners have Docker. A binary without the flag skips it with a warning. `behavior-budget`: seconds for it (default: the config's, else 600). With `fail-on: behavior`, a test that passes at the base and fails at the head fails the job.
- `cache`: `true` (the default) keeps the base commit's map in the Actions cache, so later pushes to the same pull request skip mapping the base again. `false` maps it every time.
- `feedback`: `true` (the default) ends the comment with a line asking for a 👍 or 👎 reaction and for `/onus caught` replies (see below).
- `github-token`: the token for the download and the comment. Default: the workflow's token.

Outputs: `report` (path of the JSON report), `markdown` (path of the Markdown report), `meaning-changes` and `needs-attention` (counts from the report's summary), and `metrics` (one line of JSON: changed files and lines, rows, findings, the time the report took and, with coverage, `coverage: {changed, ran}`). With `records: true` also `lane`, `review` (`waiting` or `approved` in the human lane), `provenance`, `rollout-strategy` (`none`, `canary` or `flag`) and `rollout-plan` (the path of `rollout.json`), and with `rollout-percent`, `rollout-decision` (see "Rollouts" below).

Notes:

- The pull request body is passed through an environment variable, never pasted into a script, so a body cannot inject shell commands.
- The action compares the base and head commits, not branch names: on `pull_request`, the checkout is a merge commit. Commits missing from a shallow checkout are fetched.
- On pull requests from forks, GitHub gives the workflow a read-only token, so the action skips the comment and only writes the job summary.
- The comment starts with a hidden `<!-- onus-report -->` marker, which is how the action finds its own comment to update.
- The action runs on Linux (x86_64 and ARM64), macOS and Windows runners.

## Outcome records on every pull request

With `records: true`, the action keeps the track record lanes depend on without anyone typing an outcome. The records live on a branch of the repository, `onus/records` (input `records-branch`), as three files that only grow by whole lines: `pulls.jsonl` (what Onus knew about each pull request while it was open), `outcomes.jsonl` (what happened to it) and `signals.jsonl` (what production said: incidents, rollbacks, deploys, rollout stages and traces, `onus help production`). Nothing outside the repository is needed.

    name: Onus
    on:
      pull_request:
        types: [opened, synchronize, reopened, edited, ready_for_review, closed]
      pull_request_review:
        types: [submitted, dismissed]
      issue_comment:
        types: [created]
      push:
        branches: [main]
    permissions:
      contents: write           # the onus/records branch
      pull-requests: write      # the comment, and reading reviews
      actions: write            # re-running the check after /onus approve or an approval
    jobs:
      onus:
        # Comments matter only when they are /onus commands on pull requests.
        if: >-
          github.event_name != 'issue_comment' ||
          (github.event.issue.pull_request && startsWith(github.event.comment.body, '/onus'))
        runs-on: ubuntu-latest
        steps:
          - uses: actions/checkout@v7
            with:
              fetch-depth: 0
          - uses: onushq/onus/action@v0.10.0
            with:
              records: true

What happens on each event:

- **Opened or pushed:** the change is submitted with the report as its evidence, classified with the policy, the risk inputs (including the reverts and incidents of the components it touches, from this repository's records), the hard floors and its agent setup's record, and judged when the lane is `judge` and `judge: true`. The comment shows the lane, why, the agent setup's record and an `/onus approve` command for each row that needs a person. The latest classification is appended to `pulls.jsonl`, with the components the change touched, the change's features (what the learned lane model trains on), and the judge configuration and its verdict when the judge decided. With `lanes.learned`, the model is read from the base commit, and the comment shows its risk on a **Learned risk:** line (`onus help lanes`). The change's provenance record is written to `provenance.json` (output `provenance`; `onus help provenance`), and its rollout plan to `rollout.json` (outputs `rollout-strategy` and `rollout-plan`), which the comment shows on a **Rollout:** line (`onus help production`). Production traces kept in the records (`signals.jsonl`) are passed to `onus report --traces`, so the report's map includes the calls production observed.
- **A review is submitted or dismissed:** the same as a push. The lane, the comment and the check are worked out again with the reviews as they now stand (see "Reviews in the human lane" below).
- **Merged or closed:** the pull request's latest record becomes an outcome (`merged` or `closed`), with the commit it landed as, who merged it (`mergedBy`), the components it touched and its features. Running twice records it once. The provenance record is rebuilt from the records (output `provenance`); upload it with `actions/upload-artifact` to keep it.
- **A push to the default branch:** commits that revert a merged change (`This reverts commit …`) are recorded against it; a reverted change no longer counts toward its agent setup's record.
- **A `/onus` comment** from someone with write access (owner, member or collaborator); anyone else's is answered with a 😕 and ignored:

      /onus approve new-external-service:external:acme-sms@notifications
      /onus audit ok
      /onus audit miss the opt-out was ignored
      /onus incident SMS sent twice involved: notifications, notifications:src/sms.ts#sendSms

  `approve` records a person's approval of a row and re-runs the check, so the lane updates. `audit` records a human audit of a merged change, the miss rate that says whether the automatic lanes can be trusted, and clears the change from the audit queue (`onus outcomes audits`, `onus help lanes`). `incident` marks a merged change as having caused one, with the components and symbols involved: an agent setup with an incident among its last 20 changes cannot auto-merge, and `onus outcomes backlog` lists where incidents happen. The action reacts with 👍 and replies with what it recorded. `/onus accuracy` is answered the same way but records nothing (see "Accuracy checks" below). `/onus caught <what>` from someone with write access is recorded too, as `caught: true` on the change's outcome, or on its pull record while it is open: the learned lane model counts it as a problem, like an incident (`onus help lanes`).

### Rollouts

A deploy workflow reports each stage of a rollout, and Onus decides from the change's plan and the metric samples:

    on: deployment_status
    jobs:
      rollout:
        if: github.event.deployment_status.state == 'success'
        runs-on: ubuntu-latest
        steps:
          - uses: actions/checkout@v7
          - run: ./scripts/canary-metrics.sh > "$RUNNER_TEMP/canary.json"
          - uses: onushq/onus/action@v0.10.0
            with:
              records: true
              rollout-percent: ${{ github.event.deployment.payload.percent }}
              rollout-metrics: ${{ runner.temp }}/canary.json

`rollout-change` names the change (`owner/repo#123`) or a commit; by default the deployment's commit. The step runs `onus ci rollout`, records the stage and pushes the records, and sets `rollout-decision`: `promote`, `hold`, `rollback` or `complete`. A rollback fails the step, and the change's `rolled-back` outcome is already recorded. `workflow_dispatch` with the same inputs works too. Onus never moves traffic: the workflow does, on the decision. See `onus help production`.

### Reviews in the human lane

A change in the human lane needs a person to approve it. `onus ci pr --reviews <file>` decides whether it has one, from the pull request's GitHub reviews as a JSON array:

    [{"by": "grace", "state": "APPROVED", "commit": "<sha>", "association": "MEMBER", "bot": false, "at": 1760000000}]

or as an object that also says who is in each owning team:

    {"reviews": [...], "teams": {"@acme/growth": ["grace", "lin"]}}

`by` is the login without `@`; `state` is `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED` or `PENDING`, in any case; `commit` is the commit reviewed; `association` is the reviewer's author association; `bot` and `at` (seconds) are optional. A host gets them with the GitHub CLI:

    gh api repos/OWNER/REPO/pulls/N/reviews --paginate | jq '[.[] | {by: (.user.login // "ghost"), state,
      commit: (.commit_id // ""), association: .author_association, bot: (.user.type == "Bot"),
      at: ((.submitted_at // "1970-01-01T00:00:00Z") | fromdate)}]' > reviews.json

The rules:

- Each reviewer's latest review counts, ordered by `at` and then by position. `COMMENTED` and `PENDING` never replace an earlier verdict; `DISMISSED` clears it.
- A review is not counted, and is listed in `ignored` with the reason, when the reviewer is a bot (`bot`, or a login ending in `[bot]`), opened the pull request (`--author`), has no write access (an association other than OWNER, MEMBER or COLLABORATOR), or approved a commit other than the head.
- Changes requested by a counted reviewer stand at any commit, as on GitHub.
- Whose approval counts in the human lane is `lanes.approval` in the base commit's onus.yaml, so a change cannot loosen it for itself. An explicit `--config` inside the repository is read at the base commit too; one outside it (a host's own) is taken as given. An onus.yaml that does not parse leaves the default, with a warning.
  - `owner`, the default when the change has owners: only an owner of what changed. That is a person named as an owner (`@grace`), or a member of an owning team (`@acme/growth`) when the reviews file lists the team's members. An approval from anyone else is listed in `ignored` as "not an owner". Changes requested by anyone with write access still stand.
  - `member`, the default when nobody owns what changed: any person with write access.
  - When `owner` cannot be checked, any person with write access counts, and `note` says why: an owning team whose members the file does not list, an owner given as an email, no owners at all, or no owner but the author who could approve (the only owner opened the pull request, or the owning teams hold nobody else). A team the file lists with no members has none: beside other owners it adds nobody.
- In the human lane the review is satisfied with at least one counted approval and no outstanding changes requested. Other lanes need no approval: there every approval from someone with write access is recorded, for provenance, and `approval` is `member`.

The owners are read at the base commit, as GitHub reads CODEOWNERS, so a change can name neither its own owner nor, with `owner`, its own approver. For each file of the rows that need a person (of every row when that finds nobody), they are the owners of the component holding it in the base commit's onus.yaml (or the `--config` read as above), else the owners CODEOWNERS at the base commit gives the file. The report's `hints.owners` come from the head and are not used here. They are the owners to ask, each once per pull request: the pull record keeps everyone asked so far. `onus ci owners --base <ref> --report <file> [--config <file>]` prints them, `{"owners": [...]}`, so a host can look up the members of owning teams before it runs `ci pr --reviews`.

The answer is the `review` key of what `onus ci pr` prints, always present:

    {"lane": "human", …, "review": {"required": true, "satisfied": false, "approvedBy": [],
      "changesRequestedBy": [], "owners": ["@team-growth"], "request": ["@team-growth"],
      "ignored": [{"by": "@eve", "why": "no write access (CONTRIBUTOR)"}], "approval": "owner"}}

`required` is true in the human lane only, and `satisfied` is true outside it. `approval` is the rule that counted the approvals, `owner` or `member`, and `note` is there only when `owner` fell back to `member`. `request` lists the owners to ask now: not asked before, never the author, and empty once satisfied. `lane.md` gains a **Review:** paragraph in the human lane: who was asked, or that it was approved and by whom. Without `--reviews` there are no reviews, so a human-lane change waits.

The pull record keeps `requested` (everyone asked), `reviewedBy` (the counted approvals of its head) and `approval` (the rule that counted them, once a review was needed or given), and the outcome keeps `reviewedBy` and `approval`. A human-lane outcome with no `reviewedBy` merged without a counted review: an admin merge, or a check that was not required.

With `records: true`, the action passes `--reviews`. It lists the pull request's reviews with the GitHub CLI, as above, and the members of each owning team of the repository's organisation that `onus ci owners` names. It asks the owners in `review.request` to review (GitHub's requested reviewers): `@org/team` of the repository's organisation as a team, `@login` as a person, never the author; emails and other organisations' teams are skipped. A name GitHub refuses (not a collaborator, or a team without access) is a warning, and the others are still asked. onus has already recorded it as asked, so it is not tried again on every push, and the comment says GitHub would not ask it. The workflow's own token cannot read team members (that needs the organisation's members: read permission), so with it onus counts anyone with write access for those teams and says so; give `github-token` a token that can read them to count only their members. While no review counts, the job fails (`wait-for-review`, default `true`) and the output `review` is `waiting`; once one does, it is `approved`. A `pull_request_review` event runs the action again, and after an approval it re-runs the failed `pull_request` check for the head (with `actions: write`), so both pass. Make the job a required status check in branch protection or a ruleset: otherwise a failed check is shown but does not stop a merge. A listing that fails passes no reviews, so the change keeps waiting. Each run is its own check of the same name on the head commit, and GitHub goes by the newest: when an approval is dismissed, the run for the dismissal fails, and it is the newest. For the same reason every review runs the action, comments too: a run that skipped the work would leave a passing check as the newest. Reviews on a closed pull request do nothing. Only `records: true` waits; `lanes: true` alone classifies without reviews, and says so in a notice when the lane is human. A pull request runs the workflow and the action as it changes them, so it could turn the wait off for itself: protect `.github/workflows` (and the action, when it is in the repository) with CODEOWNERS and *Require review from Code Owners*, or a ruleset. Onus Cloud passes `--reviews` too, and turns `satisfied` into its check.

### Who made the change

Each outcome is kept against an agent setup. Onus reads it from what git and GitHub show, in order:

1. An `Onus-Agent: tool/model/config` trailer on the head commit, such as `Onus-Agent: claude-code/opus/plan-first`. The most precise; agents can be told to add it.
2. A branch named after an agent: `claude/`, `claude-code/`, `codex/`, `cursor/`, `copilot/`, `devin/`, `jules/`, `aider/`, and `onus/` for changes pushed through the Onus gateway.
3. The bot that opened the pull request, such as `copilot-swe-agent` or `devin-ai-integration[bot]`; any other bot under its own name.
4. A `Co-authored-by` trailer naming an agent.
5. Otherwise the person who opened it, as `person/<login>`.

### The agent's submission

An agent that hands in its change with `onus submit` (or `onus_submit`) under a task token has a submission: the token's task, rights, expiry and fingerprint, verified when it was made, and the escalations granted to it. Give it to the action so the change's provenance keeps them: the `submission` input (a path written by an earlier step), or a file on the records branch at `submissions/<pull request number>.json` or `submissions/<head commit>.json`. `onus ci records pull` copies those into `submissions/` of the records folder, and a push keeps whatever else is on the branch. The action passes it to `onus ci pr --submission` and to `onus provenance --agent-submission` when the pull request closes.

Only the token and the escalations are taken from it, and only when it is for one of the pull request's commits. The record marks the token `agent-submission`, ranked below the door's word, and the `escalations` gap says the escalations it lists are the agent's word: the file is plain JSON, and anyone who can write to the records branch can put one there; one that cannot be read is a warning, never a failed job. The action cannot verify the token itself (a submission never keeps it, and no root key reaches CI), and it sees no door log and none of what Onus Cloud keeps (escalations, calls, the door log an agent attached there). Those stay gaps in the provenance record unless a host passes them (`onus help provenance`).

### Reading the records

`onus ui` reads `origin/onus/records` when there is no local outcomes file: the Outcomes page shows changes by week and lane, each agent setup's record and whether it may auto-merge, and the pull requests in flight. Its *Fetch from origin* button fetches the branch. On the command line:

    onus ci records pull --dir records
    onus outcomes summary --file records/outcomes.jsonl

### Notes

- Pull requests from forks get a read-only token on `pull_request` events, so they are classified but not recorded.
- Concurrent runs merge their records by taking every line from both sides, each as many times as the side that holds it most, and retry the push when another run pushed first. So a line one run wrote twice is kept twice, and a line two runs each wrote once is kept once.
- The `onus ci` commands the action runs work in any CI system: `onus ci pr`, `onus ci owners`, `onus ci closed`, `onus ci comment`, `onus ci push`, `onus ci rollout`, `onus ci rollback` and `onus ci records pull|push` (see `onus help commands` and `onus ci --help`).

## Which changed lines tests ran

With coverage of the head commit, the report says how many changed lines tests ran and lists those no test ran (`onus help reports`). The tests must run at the pull request's head commit, not the merge commit `pull_request` checks out, so check it out for them:

    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          fetch-depth: 0
      - run: npm ci && npx vitest run --coverage --coverage.reporter=lcov
      - uses: onushq/onus/action@v0.10.0
        with:
          coverage: coverage/lcov.info

When the checkout is another commit, the action leaves the coverage out with a warning. Coverage that environments recorded comes in through `evidence` (with records): the action passes those runs to `onus report --evidence` too, and only runs at the head commit count (`onus help environments`). With `lanes.inputs.coverage`, too few changed lines run sends the change to the judge, and recorded coverage counts only from the commands it names in `commands`: recorded coverage from any other run sends the change to the judge too, so name the run that records it (`onus help lanes`).

## Caching the base map

Most of a report's time goes into mapping the two commits. A pull request is reported on every push, but its base commit rarely changes, so the action caches the base map (input `cache`). On the command line, the same cache is `--cache-dir`:

    onus report --base "$BASE" --head "$HEAD" --cache-dir ~/.cache/onus

The cached map's file name is a hash of everything that shapes it: the base commit, the Onus version, the onus.yaml and its packs, the plugins file and trusted mode, and any `--base-scip` indexes. A change to any of them builds a new map; a cached file is never updated in place, and one that cannot be read is ignored. The report is byte for byte the same with or without the cache.

With a cached base map, Onus extracts only the base commit's changed files (plus `onus.yaml`, its packs and `.gitignore` files), which roughly halves the time of a report on a large repository: about 12 seconds instead of 22 for a pull request in a 31,000-file repository.

## Was the report useful? 👍 👎, /onus caught and /onus accuracy

The Phase 1 question is whether reviewers prefer the report to the raw diff. Two signals answer it, both left on the pull request itself:

- A 👍 or 👎 reaction on the Onus comment.
- A reply that starts with `/onus caught`, followed by what the report surfaced that the diff hid:

      /onus caught the new SMS vendor; I would have missed it in 1,400 lines

Anyone's `/onus caught` counts toward the catch rate. One from someone with write access is also recorded when the action keeps records (`records: true`), and trains the learned lane model. To acknowledge those replies with a 🚀, add a second workflow; the same action handles both events:

    name: Onus caught
    on:
      issue_comment:
        types: [created]
    permissions:
      issues: write
      pull-requests: write
    jobs:
      caught:
        if: github.event.issue.pull_request && startsWith(github.event.comment.body, '/onus caught')
        runs-on: ubuntu-latest
        steps:
          - uses: onushq/onus/action@v0.10.0

### Accuracy checks

Wrong facts are the metric that matters most, so people can also say whether a report was right. After reading it, reply:

    /onus accuracy ok
    /onus accuracy wrong 3 the vendor is not new
    /onus accuracy missed the nightly cron job now sends email too

`wrong` names a row by its number in the table (from 1) or by its id; `missed` says what the report left out. Only people with write access (OWNER, MEMBER, COLLABORATOR) can give an accuracy check. With `records: true`, the action answers with a 👍, or with 😕 and the reason when it cannot take the check. It records nothing: an accuracy check is not an outcome and changes no track record. Onus Cloud asks for a check on a sample of reports, keeps the answers, and shows them with the 👍 and 👎 counts, the catch rate and review times on its Metrics page.

The comment also carries the report's metrics line in a hidden HTML comment. `scripts/onus-metrics.sh` in the Onus repository reads all of it back with the GitHub CLI, one JSON line per pull request, or one line of totals with `--summary` (reports, large reports, the share of 👍 on large pull requests, the catch rate, changed lines per row, time to first review):

    scripts/onus-metrics.sh your-org/your-repo --limit 200 --summary

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
