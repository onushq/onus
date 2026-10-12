# Risk lanes and the judge

Once Onus has described a change, lanes decide what happens to it: merge it automatically, have the judge verify it, send it to a person, or block it. Lanes come from rules you write and from hard floors no rule can lower; nothing about a lane is generated.

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

The answer is the lane and every rule that applied, in order. Each entry has a `source` (`default`, `policy`, `input`, `floor`, `record` or `audit`), a reason, and the ids of the report rows behind it (`rows`):

1. The policy's `default` lane, or a lower one when a `match: every` rule covers every row of the change (for example, internal changes in a docs component).
2. Rules that raise the lane when any row matches.
3. Risk inputs (`lanes.inputs`, below). They can only raise the lane.
4. Hard floors:
   - **Blocked:** a committed secret, a write outside the token's scope, or weakened tests without a person's approval.
   - **Human:** rows in sensitive code (`auth`, `payments`, `pii`, or any label of medium or high sensitivity), security-sensitive rows, rules of the game (tests, CI, policies, `onus.yaml`, and edits to the code of a declared contract), rows outside the stated intent, and third-party code new to the repository (an npm package no component used before).
   - **Judge:** a new dependency never auto-merges: a package new to a component, or a new dependency between components.
5. An agent setup with fewer than `minRecord` merged changes on record cannot auto-merge. A share (`auditRate`) of the remaining auto-merges is sent to a person as a random audit, chosen by a hash of the head commit.

## Risk inputs

Four built-in inputs read facts Onus computes for every change. They are on by default, each can be tuned or turned off, and none can lower a lane:

| Input | Fires when | Default |
|---|---|---|
| `blastRadius` | A row changes something other components depend on (`dependentComponents` in the report). The highest lane whose threshold the count meets applies. | 3 or more components → `judge` |
| `tests` | A row changes code no test exercises and no runtime trace has observed (`tested: untested`). | `judge` |
| `confidence` | A row rests on parts of the map Onus could not resolve (`confidence: low`). | `judge` |
| `history` | A component the change touches had reverts, rollbacks or incidents in the last `days` days, read from `--outcomes`. | a revert → `judge`, an incident → `human`, over 90 days |

    lanes:
      inputs:
        blastRadius: { judge: 3, human: 10 }   # `{}` turns it off
        tests: { untested: judge }             # `auto-merge` turns it off
        confidence: { low: judge }
        history:
          days: 90
          reverts: { judge: 1 }
          incidents: { human: 1 }

Test reach is per file: a file counts as tested when a test in the map exercises anything in it, and as traced when a runtime trace saw it (`onus help environments`). It is not line coverage. History counts each change once per component, from every record in the window, so a revert and an incident count at the time they were recorded. Records shared by several repositories are scoped with `--change owner/repo#N`: only that repository's changes count, and the change itself is left out. `onus ci pr` does this for you.

## The policy

    lanes:
      default: judge
      auditRate: 0.05
      minRecord: 10
      rules:
        - { lane: auto-merge, match: every, kinds: [internal], components: [docs] }
        - { lane: human, subkinds: [migration-changed, route-added] }
      heldOut:
        command: npm run test:held-out
        image: node:22
        setup: npm ci
      taste:
        command: [./scripts/review.sh]

A rule matches a row by `kinds`, `subkinds`, `components` and `labels` (an empty list matches anything). `onus help changes` lists the kinds and subkinds.

## The judge

    onus judge --submission submission.json      # exit 0 approve, 2 reject, 3 escalate

It verifies first and judges taste last, and stops being lenient at the first failure:

1. **Evidence:** each test run in the submission is run again at the head commit in a fresh container; a run that passed for the author must pass again. A run made in an environment (`onus env run`) names its manifest, and must match it: recorded at the head commit, with the same command and exit code (`onus help environments`).
2. **Intent:** an intent must be stated, and no row may fall outside it.
3. **Contracts:** no breaking contract changes and no boundary-rule violations.
4. **Tests:** no weakened tests without a person's approval.
5. **Held-out checks:** `heldOut.command` runs in a container at the head commit. Keep it out of the agent's read scope (`onus help scopes`); a failure is reported without its output.
6. **Taste (optional):** `taste.command` gets the submission as JSON on stdin and prints `concern: …` lines. Concerns send the change to a person; taste can never approve, and it runs only when everything else passed. Onus itself calls no model.

The verdict is `approve`, `reject` with reasons the agent can act on, or `escalate` to a person. A change in the `human` lane is escalated even when every check passes; a `blocked` change is rejected.

## Outcomes

The GitHub Action keeps these records for you with `records: true`: outcomes when pull requests close, reverts when the default branch moves, and approvals, audits and incidents from `/onus` comments (`onus help ci`). By hand, or from another system:

    onus outcomes record --file outcomes.jsonl --change acme/shop#42 --agent claude-code/sonnet/default \
      --lane auto-merge --verdict approve --judge 1a2b3c4d --result merged --audited
    onus outcomes summary --file outcomes.jsonl

Pass `--component <id>` (repeatable) to name the components the change touched, for the history input. `onus ci pr` records them from the report, and `onus ci closed`, reverts and incidents keep them.

The summary gives each agent setup its record, and each judge configuration its record of the changes it decided (a verdict on record): merged, reverted, incidents, audited, missed. It also gives the share of changes in the human lane (the review budget), and the miss rate of human audits of automatic approvals, the number that says whether the automatic lanes can be trusted. A judge configuration id is a hash of the lane policy and the Onus version, so changing either starts a new record: that is how configurations are rotated and compared.

## The production loop

What happens after merging changes the record:

    onus outcomes ingest-reverts --file outcomes.jsonl --since v1.4.0     # reverts in git history
    onus outcomes incident --file outcomes.jsonl --change acme/shop#42 \
      --involved notifications --involved "notifications:src/sms.ts#sendSms" --note "SMS sent twice"
    onus outcomes backlog --file outcomes.jsonl

Reverts are found from commit messages (`This reverts commit …`) and recorded against the change they undo; a reverted change no longer counts toward its agent setup's record. A change turned off by a feature flag or a rolled-back rollout is recorded with `onus outcomes record --result rolled-back` and counts the same way. An agent setup with an incident among its last 20 changes cannot auto-merge. `backlog` lists the components and symbols incidents involved, most often first: where the next held-out tests should go. Onus does not write tests.

## In CI

In the human lane, `onus ci pr --reviews <file>` decides whether the change has the review it needs: an approval of the head commit from someone with write access who is not a bot and did not open the pull request, and no outstanding request for changes. It also names the owners of the rows that need a person (onus.yaml, else CODEOWNERS) for the host to ask, each once per pull request. Onus Cloud uses this to keep its check waiting until then; see `onus help ci`.

The action classifies every pull request with `lanes: true` and shows the lane in its comment. `apply-lane: true` labels the pull request `onus/lane:<lane>` and fails the job for `blocked`; `auto-merge: true` enables GitHub's auto-merge for the `auto-merge` lane. See `onus help ci`. With `records: true`, the action also keeps the outcome records on an `onus/records` branch: outcomes when pull requests close, reverts when the default branch moves, and approvals, audits and incidents from `/onus` comments.
