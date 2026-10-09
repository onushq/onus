# Risk lanes and the judge

Once Onus has described a change, lanes decide what happens to it: merge it automatically, have the judge verify it, send it to a person, or block it. Lanes come from rules you write and from hard floors no rule can lower; nothing about a lane is generated.

| Lane | What happens |
|---|---|
| `auto-merge` | It merges when required checks pass (the action can enable GitHub's auto-merge). |
| `judge` | `onus judge` verifies it; it merges on `approve`. |
| `human` | A person reviews it. |
| `blocked` | It may not merge as it is. |

## Submitting a change

An agent (or a person) bundles a change with what a reviewer needs and nothing of its reasoning:

    onus submit --base main --head HEAD --intent intent.yaml --evidence run.json \
      --token "$TOKEN" --key-public root.pub \
      --agent-tool claude-code --agent-model sonnet --agent-config default --out submission.json

The submission holds the stated intent, Onus's report, every changed path, the test runs from `onus run-test` (to be re-run), the token's verified scope, granted escalations, approvals (`--approve <row id>=<who>`), and the agent setup.

## Classifying

    onus classify --submission submission.json --outcomes outcomes.jsonl
    onus classify --report report.json              # a change without a submission (CI)

The answer is the lane and every rule that applied, in order:

1. The policy's `default` lane, or a lower one when a `match: every` rule covers every row of the change (for example, internal changes in a docs component).
2. Rules that raise the lane when any row matches.
3. Hard floors:
   - **Blocked:** a committed secret, a write outside the token's scope, or weakened tests without a person's approval.
   - **Human:** rows in sensitive code (`auth`, `payments`, `pii`, or any label of medium or high sensitivity), security-sensitive rows, rules of the game (tests, CI, policies, `onus.yaml`, declared contracts), and rows outside the stated intent.
4. An agent setup with fewer than `minRecord` merged changes on record cannot auto-merge. A share (`auditRate`) of the remaining auto-merges is sent to a person as a random audit, chosen by a hash of the head commit.

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

The summary gives each agent setup and judge configuration its record (merged, reverted, incidents, audited, missed), the share of changes in the human lane (the review budget), and the miss rate of human audits of automatic approvals, the number that says whether the automatic lanes can be trusted.

## The production loop

What happens after merging changes the record:

    onus outcomes ingest-reverts --file outcomes.jsonl --since v1.4.0     # reverts in git history
    onus outcomes incident --file outcomes.jsonl --change acme/shop#42 \
      --involved notifications --involved "notifications:src/sms.ts#sendSms" --note "SMS sent twice"
    onus outcomes backlog --file outcomes.jsonl

Reverts are found from commit messages (`This reverts commit …`) and recorded against the change they undo; a reverted change no longer counts toward its agent setup's record. A change turned off by a feature flag or a rolled-back rollout is recorded with `onus outcomes record --result rolled-back` and counts the same way. An agent setup with an incident among its last 20 changes cannot auto-merge. `backlog` lists the components and symbols incidents involved, most often first: where the next held-out tests should go. Onus does not write tests.

## In CI

The action classifies every pull request with `lanes: true` and shows the lane in its comment. `apply-lane: true` labels the pull request `onus/lane:<lane>` and fails the job for `blocked`; `auto-merge: true` enables GitHub's auto-merge for the `auto-merge` lane. See `onus help ci`. With `records: true`, the action also keeps the outcome records on an `onus/records` branch: outcomes when pull requests close, reverts when the default branch moves, and approvals, audits and incidents from `/onus` comments.
