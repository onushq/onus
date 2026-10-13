# Risk lanes and the judge

Once Onus has described a change, lanes decide what happens to it: merge it automatically, have the judge verify it, send it to a person, or block it. Lanes come from rules you write and from hard floors no rule can lower. A learned model you train and commit can raise a lane, never lower it.

| Lane | What happens |
|---|---|
| `auto-merge` | It merges when required checks pass (the action can enable GitHub's auto-merge). |
| `judge` | `onus judge` verifies it; it merges on `approve`. |
| `human` | A person with write access who did not open it approves its head commit. Its owners are asked to review. |
| `blocked` | It may not merge as it is. |

## Submitting a change

An agent (or a person) bundles a change with what a reviewer needs and nothing of its reasoning:

    onus submit --base main --head HEAD --intent intent.yaml --evidence run.json \
      --token "$TOKEN" --key-public root.pub \
      --agent-tool claude-code --agent-model sonnet --agent-config default --out submission.json

The submission holds the stated intent, Onus's report, every changed path, the test runs from `onus run-test` (to be re-run), the token's verified scope, granted escalations (`--escalation <id>`, each verified with `--key-public`: granted, for the token's task, unchanged since it was filed; requests are read from `.onus/escalations`, or `--escalations-dir`), approvals (`--approve <row id>=<who>`), and the agent setup. When the token names an agent, that is the agent setup, whatever the flags say.

## Classifying

    onus classify --submission submission.json --outcomes outcomes.jsonl
    onus classify --report report.json              # a change without a submission (CI)
    onus classify --submission submission.json --outcomes outcomes.jsonl --change acme/shop#42

The answer is the lane and every rule that applied, in order. Each entry has a `source` (`default`, `policy`, `input`, `floor`, `model`, `record` or `audit`), a reason, and the ids of the report rows behind it (`rows`):

1. The policy's `default` lane, or a lower one when a `match: every` rule covers every row of the change (for example, internal changes in a docs component).
2. Rules that raise the lane when any row matches.
3. Risk inputs (`lanes.inputs`, below). They can only raise the lane.
4. Hard floors:
   - **Blocked:** a committed secret, a write outside the token's scope, or weakened tests without a person's approval.
   - **Human:** rows in sensitive code (`auth`, `payments`, `pii`, or any label of medium or high sensitivity), security-sensitive rows, rules of the game (tests, CI, policies, `onus.yaml`, and edits to the code of a declared contract), rows outside the stated intent, and third-party code new to the repository (an npm package no component used before).
   - **Judge:** a new dependency never auto-merges: a package new to a component, or a new dependency between components.
5. The learned model (`lanes.learned`, below), read from the base commit: at or above its thresholds it raises a lower lane to `judge` or `human`. It never lowers a lane and never blocks.
6. An agent setup with fewer than `minRecord` merged changes on record, or with an incident or a rollback among its recent changes, cannot auto-merge. A share (`auditRate`) of the remaining auto-merges is sent to a person before merging, chosen by a hash of the head commit.

Every classification also carries the change's `rollout` plan for its lane: how it should meet production traffic, and when it rolls back (`onus help production`).

## Risk inputs

Four built-in inputs read facts Onus computes for every change. They are on by default, each can be tuned or turned off, and none can lower a lane. A fifth, `coverage`, is off unless written:

| Input | Fires when | Default |
|---|---|---|
| `blastRadius` | A row changes something other components depend on (`dependentComponents` in the report), including a body-only edit of a function they call. The highest lane whose threshold the count meets applies. | 3 or more components → `judge` |
| `tests` | A row changes code no test exercises and no runtime trace has observed (`tested: untested`). | `judge` |
| `confidence` | A row rests on parts of the map Onus could not resolve (`confidence: low`). | `judge` |
| `history` | A component the change touches had reverts, rollbacks or incidents in the last `days` days, read from `--outcomes` and the production signals beside it (`signals.jsonl`). A planned rollback does not count. | a revert → `judge`, an incident → `human`, over 90 days |
| `coverage` | Tests ran less than `minChanged` (0 to 1) of the change's changed executable lines, from coverage of the head (`onus help reports`), or the coverage leaves out a changed file with runtime code; with `required: true`, also a change without coverage. | off |

    lanes:
      inputs:
        blastRadius: { judge: 3, human: 10 }   # `{}` turns it off
        tests: { untested: judge }             # `auto-merge` turns it off
        confidence: { low: judge }
        history:
          days: 90
          reverts: { judge: 1 }
          incidents: { human: 1 }
        coverage: { minChanged: 0.8, commands: ["npm test -- --coverage"] }   # off unless written

Every classification also carries `features`: what the change looked like when it was classified, which the learned model trains on, and `learned` when `lanes.learned` is set.

Test reach is per file: a file counts as tested when a test in the map exercises anything in it, and as traced when a runtime trace saw it (`onus help environments`). It is not line coverage; `coverage` is. Coverage comes from `onus report --coverage` or `--evidence`, or, for a submission made without a report (`onus ci pr`, `onus_submit`), from the coverage its evidence runs recorded at the head commit (`onus help environments`). The `coverage` input runs only while a change would auto-merge. It counts recorded coverage only from the commands listed in `commands`, since an agent picks the commands it runs and one could write a coverage file; when the report holds recorded coverage from any other command, the change goes to the judge, required or not. Coverage given with `--coverage` counts. History counts each change once per component, from every record in the window, so a revert and an incident count at the time they were recorded. Code outside every component counts as `root`. A backfill (`onus ci pr --at`) sees only what was recorded by then. Records shared by several repositories are scoped with `--change owner/repo#N`: only that repository's changes count, and the change itself is left out. `onus ci pr` does this for you.

## A learned model that only raises

Once outcome records show which changes went wrong, a model can learn from them. It may move a change up a lane, never below the lane the rules give it, and never to `blocked`.

    onus ci records pull --dir rec
    onus lanes train --outcomes rec/outcomes.jsonl          # writes onus.lanes-model.json
    git add onus.lanes-model.json && git commit -m "Lanes model"

    lanes:
      learned:
        model: onus.lanes-model.json   # the default
        judge: 0.6                     # raise to the judge at or above this risk
        human: 0.85                    # raise to a person at or above this risk

**What it learns from.** Every classification computes a fixed set of features from the report (rows of each kind, blast radius, dependent components, lines and files, novelty, sensitive code, rows that need a person, map confidence, test reach, unverified contracts, rule violations, a stated intent), the submission (passing and failing test runs) and the records (earlier changes and problems in the same components, the agent setup's record). `onus ci pr` keeps them on the pull record, `onus ci closed` copies them onto the outcome, and reverts, incidents and audits keep them. `onus outcomes record --classification <file>` copies them from a classification, for outcomes recorded by hand.

**What counts as a problem.** A change that had an incident, was reverted or rolled back (a planned rollback is not a problem: it counts as a clean merge), whose audit found a miss, or about which a person with write access said `/onus caught` (`onus help ci`). A merged change counts as clean once it is `--settle-days` old (default 14) at the latest record's time, so reverts and incidents have had time to show. Open changes, changes closed without a catch, unsettled merges and records without features are left out, and the output says how many.

**Training.** A logistic regression on standardized features, class-balanced and L2-regularised, fitted by gradient descent from zero with a fixed number of steps. There is no randomness: the same records in any line order give the same file, byte for byte. A fifth of the changes, chosen by a hash of their ids, is held out: a first fit without them is measured on them, then the model is fitted on everything. It refuses with "too little data" (exit 1) below `--min-examples` (default 50) labelled changes or `--min-positives` (10) problems, or when the part left after holding out the fifth lacks problems or clean merges. With `--config` and `--out`, nothing is read from `--repo`, so it runs outside a repository. `--dry-run` measures and writes nothing. It prints:

    { "out": "onus.lanes-model.json", "written": true, "model": "3f2a9c1e0b7d",
      "examples": 412, "positives": 23,
      "skipped": { "noFeatures": 80, "staleFeatures": 0, "unsettled": 12, "undecided": 35 },
      "holdout": { "examples": 83, "positives": 5, "auc": 0.82,
                   "atJudge": { "raised": 0.21, "recall": 0.8 }, "atHuman": { "raised": 0.04, "recall": 0.4 } },
      "weights": [ { "feature": "kind.breaking", "weight": 1.21 }, … ] }

`raised` is the share of held-out changes at or above a threshold, and `recall` the share of held-out problems at or above it. Tune `judge` and `human` with them: lower thresholds raise more changes and catch more problems. The risk is a relative score, not a calibrated probability, because the classes are balanced.

**Applying it.** `onus classify`, `onus judge`, `onus ci pr`, `onus ui` and the MCP `onus_submit` tool read the model from the base commit (the submission's base; the working tree for `onus classify --report`), so a pull request cannot delete or retrain the model to avoid a raise. `--model <file>` hands in another one. A raise is listed with `source: "model"`:

    model → human: the learned model puts this change's risk at 0.91 (a person from 0.85, the judge from 0.60): 2 breaking rows (+1.42), blast radius 38 (+0.97), 3 earlier problems in billing (+0.61); model 3f2a9c1e0b7d, trained on 412 outcomes

When it does not raise, `learned` still holds the risk and its top factors, and the CI comment shows a **Learned risk:** line. A model file that is missing at the commit, was trained on another feature set (`stale`), or does not parse (`invalid`) is never an error: `learned.status` and `learned.note` say why, and the lane comes from rules only.

**Rules of the game.** The model file and `onus.yaml` decide lanes, so a pull request that changes either goes to a person (`lanes-model-changed`, `onus-config-changed`). Retrain after a release that adds features: an older model reports `stale` until then.

**What it is not.** Outcomes are shaped by the lanes changes got: a change a person reviewed was less likely to go wrong. The model learns what went wrong despite the lanes, which is the right target for raising a lane, but not a causal estimate of risk.

## The policy

    lanes:
      default: judge
      auditRate: 0.05
      auditAfterMerge: 0.05
      minRecord: 10
      rules:
        - { lane: auto-merge, match: every, kinds: [internal], components: [docs] }
        - { lane: human, subkinds: [migration-changed, route-added] }
      taste:
        command: [./scripts/review.sh]
      judges:
        - name: claude-reviewer                         # uses taste above
        - name: gpt-reviewer
          taste: { command: [./scripts/review-gpt.sh] }
        - name: strict
          heldOutLimit: 20

A rule matches a row by `kinds`, `subkinds`, `components` and `labels` (an empty list matches anything). `onus help changes` lists the kinds and subkinds. Held-out checks are not in the policy: they live outside the repository (below).

## The judge

    onus judge --submission submission.json      # exit 0 approve, 2 reject, 3 escalate
    onus judge --submission submission.json --held-out ~/acme-held-out/shop

It verifies first and judges taste last, and stops at the first failure:

1. **Evidence:** each test run in the submission is run again at the head commit in a fresh container; a run that passed for the author must pass again. A run made in an environment (`onus env run`) names its manifest, and must match it: recorded at the head commit, with the same command and exit code (`onus help environments`). The first run that fails stops the step; the rest are not re-run.
2. **Intent:** an intent must be stated, and no row may fall outside it.
3. **Contracts:** no breaking contract changes and no boundary-rule violations.
4. **Tests:** no weakened tests without a person's approval.
5. **Held-out checks:** checks the author never saw, from a suite outside the repository, chosen from the map for this change (below). They run in order, most relevant first, up to the first failure. The author sees only counts and `a held-out check failed (exit N)`: never a check's id, command or output.
6. **Taste (optional):** `taste.command` gets the submission as JSON on stdin and prints `concern: …` lines. Concerns send the change to a person; taste can never approve, and it runs only when everything else passed. Onus itself calls no model.

Once a step fails, every later step is `skipped` with `not reached: <step> failed`, and no more containers start. So `reasons` holds one failing step's reasons: the first thing to fix. Run `onus_check` (or `onus query check`) before submitting for the whole checklist.

The verdict is `approve`, `reject` with reasons the agent can act on, or `escalate` to a person. A change in the `human` lane is escalated even when every check passes; a `blocked` change is rejected.

### Held-out suites

A suite is a directory holding `held-out.yaml` and the files its checks need. Keep it outside the repository: in a private repository, a CI secret or Onus Cloud. The judge refuses a suite inside the repository's worktree, where the author could read it.

    version: 1
    image: node:22-alpine        # default image for checks (default node:22)
    setup: npm ci                # default setup, run with network and without the suite
    limit: 6                     # at most this many checks per change, most relevant first
    checks:
      - id: sms-respects-opt-out
        command: node --test /held-out/sms-opt-out.test.mjs
        covers: [notifications, user-preferences]
      - id: send-sms-unit
        command: node --test /held-out/send-sms.test.mjs
        covers: ["notifications:src/sms.ts#sendSms"]
      - id: smoke
        command: npm run build   # no covers: runs for every change

Each check runs in a container at the head commit with no network, the repository at `/work` (the working directory) and the suite at `/held-out`. The suite is copied into the test container only, never into the setup container, which has network. Ids are unique slugs; unknown fields are refused.

Give it to the judge with `--held-out DIR` or `ONUS_HELD_OUT`: on `onus judge`, `onus ci pr`, `onus ui` and `onus mcp` (for `onus_judge`). Two commands help people write one, without a container engine:

    onus held-out check ~/acme-held-out/shop --repo .              # valid? its hash, ids and covers
    onus held-out select --held-out ~/acme-held-out/shop --submission sub.json   # what would run, and why

**Chosen from the map.** The judge works out what the change reaches in the head map: the components of its rows and of the files git says changed (`touched`), its rows' subjects, and the pairs of components that interact with at least one side touched: a code dependency either way (a file import counts, so a change that breaks the symbols between two components still pairs them), or one publishes an event the other consumes. A `covers` entry is a component id, or a map id standing for its component; `event:` and `db-table:` ids count only as subjects. A check is chosen for its best reason:

| reason | when |
|---|---|
| `combination` | two components it covers form a pair: the cross-feature test an author's own suite rarely has |
| `subject` | a map id it covers is a row's subject, or the module of one |
| `touched` | it covers a touched component |
| `always` | it covers nothing (and `lanes.heldOut`) |
| `neighbour` | it covers a component that interacts with a touched one |

Within a reason checks keep their order in the file. The configuration's `heldOutLimit`, else the suite's `limit`, cuts the list. The head map is built only when some check has `covers`, with the same onus.yaml as the lanes; if it cannot be built, the rows alone decide, and the report says `"map": false`.

**Who sees what.** The judgment carries `heldOut: {suite, selected, ran, passed}`, safe for the author. What each check did (`why`, `with`, status, exit code and output) is for people: `onus judge --show-held-out`, `held-out.json` beside `judgment.json` from `onus ci pr`, and the lanes page of `onus ui`. No agent tool returns it, and `onus_lanes` shows only that held-out checks are configured. When a check cannot run at all (an image that will not pull, say), the author reads only that the held-out checks could not run; the error is in the report.

**The honest limit.** On a machine where an agent has a shell, a suite is only as hidden as that machine's file permissions. So is the report: `onus ui` and `onus mcp` keep it as `.onus/submissions/<id>.held-out.json` in the worktree, where such an agent can read it. Real secrecy comes from where the suite lives and where the judge runs: a CI secret, a private repository cloned with its own token, or Onus Cloud.

`lanes.heldOut` in onus.yaml still works, as one check with id `lanes.heldOut` that runs for every change. Agents can read onus.yaml, so `onus judge` notes it on stderr; prefer a suite.

### Judge configurations

With `lanes.judges` set, one configuration judges each change. Each has a `name`, and optionally its own `taste` reviewer (default `lanes.taste`) and `heldOutLimit`. The rotation picks one from a hash of the head commit and the suite's full hash, so an author who cannot read the suite cannot predict which configuration will judge a commit. Without a suite the choice is predictable from the head commit.

The judgment records `rotation: {config, index, of, forced}`, and its judge id is `<name>@<hash>`, so outcome records and `onus outcomes summary`'s `byJudge` track each configuration by name. `onus judge --judge-config <name>` (and the lanes page of `onus ui`) forces one, for example to judge an audited change again under another configuration; that is recorded as `forced: true`. Agents cannot choose. Without `judges` nothing changes, and the judge id stays the policy's hash.

Agents do not see which configuration judged. `onus_judge` leaves out `rotation`, and its judge id is the policy's hash, the same for every configuration; the judgment kept beside the submission has both, for people. Otherwise an agent could amend its commit, judge it again, and repeat until a lenient configuration came up. That is the limit: an author who sees the configuration can still shop by amending. `lane.md` from `onus ci pr` names it, so in CI an author who reads the pull-request comment can push again until it likes the one it gets, and on a machine where an agent has a shell, it can read the kept judgment. What still holds there is the record: `byJudge` shows what each configuration let through, so a lenient one shows up in its reverts, incidents and audit misses.

## Outcomes

The GitHub Action keeps these records for you with `records: true`: outcomes when pull requests close, reverts when the default branch moves, and approvals, audits and incidents from `/onus` comments (`onus help ci`). By hand, or from another system:

    onus outcomes record --file outcomes.jsonl --change acme/shop#42 --agent claude-code/sonnet/default \
      --lane auto-merge --verdict approve --judge 1a2b3c4d --result merged --audited
    onus outcomes summary --file outcomes.jsonl

Pass `--component <id>` (repeatable) to name the components the change touched, for the history input, and `--classification <file>` to keep the change's features for the learned model. `onus ci pr` records them from the report, and `onus ci closed`, reverts and incidents keep them.

The summary gives each agent setup its record, and each judge configuration its record of the changes it decided (a verdict on record): merged, reverted, incidents, audited, missed. It also gives the share of changes in the human lane (the review budget), and the miss rate of human audits of automatic approvals, the number that says whether the automatic lanes can be trusted. A judge configuration id is a hash of the lane policy and the Onus version, so changing either starts a new record; with `lanes.judges`, each configuration keeps its own record as `<name>@<hash>`.

## Audits after merge

Some changes merge with no person looking: the `auto-merge` lane, and the `judge` lane when the judge approves. A share of them (`auditAfterMerge`, default 0.05) is sampled for a person to audit after they merge. `onus ci closed` makes the choice from a hash of the merge commit, which does not exist until the merge, so an agent cannot know beforehand whether its change will be audited. The choice is written into the outcome as `"sampled": true` and nowhere else: not in the comment, the check or the CI log.

`auditRate` is a different draw: it sends would-be auto-merges to a person before they merge. Set either to 0 to turn it off.

    onus outcomes audits --file outcomes.jsonl          # what waits for a person, and the audit log
    onus outcomes audit --file outcomes.jsonl --change acme/shop#42 --by @alice \
      --result miss --note "the opt-out was ignored"

`audits` prints `{awaiting, audited}`: the sampled changes no one audited yet, oldest first, and every audited change, most recent first. A sampled change that was later reverted or caused an incident still waits; leaving out the ones that went badly would bias the sample. `audit` records who audited, when, ok or a miss, and a note. `/onus audit ok|miss` on a merged pull request writes the same record (`onus help ci`).

The summary's `sampledMissRate` is the number to watch: misses among the sampled changes a person audited. Because the sample is random, it estimates how many risky changes slip through without a person. `auditMissRate` counts every audit, including the ones people chose to make, and `awaitingAudit` counts what is still waiting.

## The production loop

What happens after merging changes the record:

    onus outcomes ingest-reverts --file outcomes.jsonl --since v1.4.0     # reverts in git history
    onus outcomes incident --file outcomes.jsonl --change acme/shop#42 \
      --involved notifications --involved "notifications:src/sms.ts#sendSms" --note "SMS sent twice"
    onus outcomes backlog --file outcomes.jsonl

Reverts are found from commit messages (`This reverts commit …`) and recorded against the change they undo; a reverted change no longer counts toward its agent setup's record. A change turned off by a feature flag or a rolled-back rollout is recorded with `onus ci rollback`, `onus ci rollout` or `onus outcomes record --result rolled-back` and counts the same way, unless it was `--planned`: a planned rollback still counts as a merged change. An agent setup with an incident or a rollback among its last 20 changes cannot auto-merge. Incidents and rollbacks also come straight from production: `onus signals ingest` records signals from monitoring and deploy tools and turns those tied to a change into its outcomes (`onus help production`). `backlog` lists the components and symbols incidents involved, most often first: where the next held-out tests should go. Onus does not write tests.

## Provenance

Every change carries one record of why it was allowed through: the intent, the agent and model, the token it held, the evidence, any escalations, the lane and the judge's steps, who or what approved it, and how it rolled out. `onus ci pr` writes it as `provenance.json` beside the submission while the pull request is open. `onus provenance` rebuilds it from the records as outcomes arrive:

    onus provenance --change acme/shop#7 --records rec --from out/provenance.json --out p.json
    onus provenance --submission-id 4fa91c        # a submission kept by onus ui or onus mcp
    onus provenance --submission sub.json --audit audit.jsonl --escalations escalations --task sms
    onus provenance --verify p.json

`--from` is an earlier record of the change. Its facts from while the change was open (intent, report, token, evidence, escalations, lane reasons, judge, doors) are kept unless fresh ones are given. The stage, merge commit, approver and rollout are worked out again from the records every time. `--submission-id` reads `.onus/submissions/<id>.json`, its judgment, and the repository's `.onus` audit log, escalations and outcomes.

| Field | What it holds |
|---|---|
| `stage` | `open`, `merged` or `closed`. |
| `intent` | Where the intent came from (`pull-request` or `submission`), its sha256, and the report's intent check. |
| `agent` | The agent setup, how it was recognised (`trailer`, `branch`, `bot`, `co-author`, `person`, `submission`), and its track record (`merged`, `recentIncidents`, `recentRollbacks`). |
| `token` | The task, rights, attenuations, expiry and a 16-digit fingerprint. Never the token itself. `source` says where it came from: `token` (verified by this run), `submission` (verified when the submission was made) or `audit` (the door log's `mint` entry, widened by grants). |
| `evidence` | Each test run: command, commit, image, exit code, manifest, and whether it ran at the head commit. |
| `escalations` | Each request: kind, scopes, best evidence grade, and the last decision in the door log, by whom (`policy` for an automatic grant). |
| `lane`, `judge` | The final lane, every rule that applied, the lane policy's hash, and the judge's verdict, steps and reasons. |
| `approval` | Who or what approved it (below), the people, the rows they approved, and who merged it. |
| `rollout` | The result now, the merge commit and time, whether a person audited it, and every outcome in order. From production: `status` (`pending`, `not-shipped`, `awaiting-deploy`, `in-progress`, `holding`, `complete`, `rolled-back` or `reverted`), the `percent` of traffic last reported, the `plan`, every deploy and rollout stage (`stages`) and the other signals tied to it (`signals`). `onus help production` has the rules. |
| `doors` | The door log's decisions for the task, and whether its hash chain is intact. |
| `gaps` | What the record cannot vouch for, and how to fill it. |

Who or what approved it:

| Stage | Final lane | Judge verdict | `approval.by` |
|---|---|---|---|
| open | any | any | `pending` |
| closed | any | any | `none` |
| merged | `auto-merge` | any | `policy` |
| merged | `judge` | `approve` | `judge` |
| merged | `judge` | `escalate`, `reject` or none | `person` |
| merged | `human` | any | `person` |
| merged | `blocked` | any | `override` (it merged although Onus blocked it) |

`approval.people` lists the people who approved rows, the approving reviewers, then who merged it (`onus ci closed --merged-by`), each once. When a person approved and none is known, `gaps` says so.

A gap is listed for each of `intent`, `token`, `evidence`, `escalations`, `judge` and `doors` that is empty. In CI no task token reaches the run, so `token` is a gap there.

`digest` is the sha256 of the record's compact JSON with the digest left empty. `--verify` checks it and the schema version, and exits 1 when either does not match. The format is `schemas/provenance.schema.json`; new optional fields may be added under `schema: 1`.

## In CI

In the human lane, `onus ci pr --reviews <file>` decides whether the change has the review it needs: an approval of the head commit from someone with write access who is not a bot and did not open the pull request, and no outstanding request for changes. It also names the owners of the rows that need a person (onus.yaml, else CODEOWNERS) for the host to ask, each once per pull request. Onus Cloud uses this to keep its check waiting until then; see `onus help ci`.

The action classifies every pull request with `lanes: true` and shows the lane in its comment. `apply-lane: true` labels the pull request `onus/lane:<lane>` and fails the job for `blocked`; `auto-merge: true` enables GitHub's auto-merge for the `auto-merge` lane. See `onus help ci`. With `records: true`, the action also keeps the outcome records on an `onus/records` branch: outcomes when pull requests close, reverts when the default branch moves, and approvals, audits and incidents from `/onus` comments.
